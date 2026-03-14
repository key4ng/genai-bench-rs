use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use indicatif::{ProgressBar, ProgressStyle};
use tokio::sync::{mpsc, Semaphore};

use crate::client::BenchmarkClient;
use crate::metrics::{
    aggregate_metrics, compute_request_metrics, AggregatedMetrics, RawRequestResult, RequestMetrics,
};
use crate::scenario::Scenario;
use crate::tokenizer::PromptGenerator;

const WARMUP_SECS: f64 = 5.0;
const COOLDOWN_SECS: f64 = 5.0;

pub struct RunConfig {
    pub duration: Duration,
    pub concurrency: u32,
}

pub struct RunResult {
    pub aggregated: AggregatedMetrics,
    pub all_requests: Vec<RawRequestResult>,
    pub error_breakdown: std::collections::HashMap<String, usize>,
}

pub async fn run_benchmark(
    client: Arc<BenchmarkClient>,
    prompt_generator: Arc<PromptGenerator>,
    scenario: &dyn Scenario,
    config: &RunConfig,
    cancelled: Arc<AtomicBool>,
) -> RunResult {
    let (tx, mut rx) = mpsc::channel::<RawRequestResult>(config.concurrency as usize * 2);
    let semaphore = Arc::new(Semaphore::new(config.concurrency as usize));
    let request_counter = Arc::new(AtomicU64::new(0));
    let completed_counter = Arc::new(AtomicU64::new(0));

    let run_start = Instant::now();
    let duration = config.duration;

    // Progress bar
    let pb = ProgressBar::new(duration.as_secs());
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{msg} [{bar:40}] {pos}/{len}s {prefix} reqs [{eta} left]")
            .unwrap()
            .progress_chars("█░"),
    );
    pb.set_message(format!("Concurrency {}", config.concurrency));

    let error_counter = Arc::new(AtomicU64::new(0));
    const MAX_ERROR_LOGS: u64 = 3;

    // Spawn request producer
    let producer_client = client.clone();
    let producer_pg = prompt_generator.clone();
    let producer_sem = semaphore.clone();
    let producer_tx = tx.clone();
    let producer_counter = request_counter.clone();
    let producer_completed = completed_counter.clone();
    let producer_cancelled = cancelled.clone();
    let producer_errors = error_counter.clone();
    // For D(N,M) scenario.sample() always returns the same values,
    // but we call it per-request to support future distribution scenarios.
    let scenario_input = scenario.sample().0;
    let scenario_output = scenario.sample().1;

    let producer = tokio::spawn(async move {
        loop {
            if producer_cancelled.load(Ordering::Relaxed) {
                break;
            }
            if run_start.elapsed() >= duration {
                break;
            }

            let permit = match producer_sem.clone().acquire_owned().await {
                Ok(p) => p,
                Err(_) => break,
            };

            let request_id = producer_counter.fetch_add(1, Ordering::Relaxed);
            let client = producer_client.clone();
            let pg = producer_pg.clone();
            let tx = producer_tx.clone();
            let completed = producer_completed.clone();
            let errors = producer_errors.clone();
            let input_tokens = scenario_input;
            let output_tokens = scenario_output;

            tokio::spawn(async move {
                let prompt = pg
                    .generate_prompt(input_tokens)
                    .unwrap_or_else(|_| "Hello".to_string());

                let result = client
                    .send_request(request_id, &prompt, output_tokens, run_start)
                    .await;

                if result.error.is_none() {
                    completed.fetch_add(1, Ordering::Relaxed);
                } else if let Some(ref err) = result.error {
                    let count = errors.fetch_add(1, Ordering::Relaxed);
                    if count < MAX_ERROR_LOGS {
                        let msg = if err.code > 0 {
                            format!(
                                "[WARN] Request {}: HTTP {} {}",
                                request_id,
                                err.code,
                                err.message.lines().next().unwrap_or("")
                            )
                        } else {
                            format!("[WARN] Request {}: {}", request_id, err.message)
                        };
                        eprintln!("{}", msg);
                    }
                }

                let _ = tx.send(result).await;
                drop(permit);
            });
        }
    });

    // Drop the producer's tx clone so rx completes when producer is done
    drop(tx);

    // Collect results
    let mut all_results: Vec<RawRequestResult> = Vec::new();
    while let Some(result) = rx.recv().await {
        all_results.push(result);
        pb.set_position(run_start.elapsed().as_secs());
        pb.set_prefix(format!("{}", completed_counter.load(Ordering::Relaxed)));
    }

    producer.await.ok();
    pb.finish_and_clear();

    let total_errors_logged = error_counter.load(Ordering::Relaxed);
    if total_errors_logged > MAX_ERROR_LOGS {
        eprintln!(
            "[WARN] ... and {} more errors ({} total)",
            total_errors_logged - MAX_ERROR_LOGS,
            total_errors_logged
        );
    }

    let total_elapsed_ns = run_start.elapsed().as_nanos() as u64;

    // Separate successes and errors
    let mut successes: Vec<(RequestMetrics, u64, u64)> = Vec::new(); // (metrics, start_ns, end_ns)
    let mut error_breakdown: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();

    for raw in &all_results {
        if let Some(ref err) = raw.error {
            let key = if err.code > 0 {
                err.code.to_string()
            } else {
                "timeout".to_string()
            };
            *error_breakdown.entry(key).or_default() += 1;
        } else if let Some(metrics) = compute_request_metrics(raw) {
            successes.push((metrics, raw.start_ns, raw.end_ns));
        }
    }

    // Filter warmup and cooldown based on request start_time relative to run start.
    // Skip filtering if the run is too short (< warmup + cooldown + 1s of useful data).
    let warmup_ns = (WARMUP_SECS * 1_000_000_000.0) as u64;
    let cooldown_ns = (COOLDOWN_SECS * 1_000_000_000.0) as u64;
    let min_run_for_filtering = warmup_ns + cooldown_ns + 1_000_000_000; // at least 1s of data

    let filtered_with_times: Vec<(RequestMetrics, u64, u64)> =
        if total_elapsed_ns > min_run_for_filtering {
            let cooldown_threshold_ns = total_elapsed_ns.saturating_sub(cooldown_ns);
            successes
                .into_iter()
                .filter(|(_, s_ns, _)| *s_ns >= warmup_ns && *s_ns <= cooldown_threshold_ns)
                .collect()
        } else {
            // Run too short for warmup/cooldown — include all requests
            successes
        };

    // Compute run_duration from the filtered window:
    // from first included request's start to last included request's end
    let run_duration = if filtered_with_times.is_empty() {
        run_start.elapsed().as_secs_f64()
    } else {
        let first_start = filtered_with_times
            .iter()
            .map(|(_, s, _)| *s)
            .min()
            .unwrap();
        let last_end = filtered_with_times
            .iter()
            .map(|(_, _, e)| *e)
            .max()
            .unwrap();
        ((last_end - first_start) as f64 / 1_000_000_000.0).max(0.001)
    };

    let filtered: Vec<RequestMetrics> =
        filtered_with_times.into_iter().map(|(m, _, _)| m).collect();

    let error_count = error_breakdown.values().sum::<usize>();

    let mut aggregated = if filtered.is_empty() {
        AggregatedMetrics::empty()
    } else {
        aggregate_metrics(&filtered, run_duration)
    };

    aggregated.concurrency = config.concurrency;
    aggregated.error_count = error_count;
    aggregated.error_rate = if all_results.is_empty() {
        0.0
    } else {
        error_count as f64 / all_results.len() as f64
    };

    RunResult {
        aggregated,
        all_requests: all_results,
        error_breakdown,
    }
}
