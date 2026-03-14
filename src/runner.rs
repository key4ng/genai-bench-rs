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
    pub max_requests: Option<u64>,
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
    let max_requests = config.max_requests;

    // Progress bar
    let pb = ProgressBar::new(max_requests.unwrap_or(duration.as_secs()));
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{msg} [{bar:40}] {pos}/{len} reqs [{eta} left]")
            .unwrap()
            .progress_chars("##-"),
    );
    pb.set_message(format!("Concurrency {}", config.concurrency));

    // Spawn request producer
    let producer_client = client.clone();
    let producer_pg = prompt_generator.clone();
    let producer_sem = semaphore.clone();
    let producer_tx = tx.clone();
    let producer_counter = request_counter.clone();
    let producer_completed = completed_counter.clone();
    let producer_cancelled = cancelled.clone();
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
            if let Some(max) = max_requests {
                if producer_completed.load(Ordering::Relaxed) >= max {
                    break;
                }
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
                    // Real-time per-request error warning
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
        pb.set_position(completed_counter.load(Ordering::Relaxed));
    }

    producer.await.ok();
    pb.finish_and_clear();

    let total_elapsed_ns = run_start.elapsed().as_nanos() as u64;

    // Separate successes and errors
    let mut successes: Vec<(RequestMetrics, u64)> = Vec::new(); // (metrics, run_offset_ns)
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
            successes.push((metrics, raw.run_offset_ns));
        }
    }

    // Filter warmup and cooldown based on request start_time relative to run start
    let warmup_ns = (WARMUP_SECS * 1_000_000_000.0) as u64;
    let cooldown_threshold_ns =
        total_elapsed_ns.saturating_sub((COOLDOWN_SECS * 1_000_000_000.0) as u64);

    let filtered: Vec<RequestMetrics> = successes
        .into_iter()
        .filter(|(_, run_offset_ns)| {
            *run_offset_ns >= warmup_ns && *run_offset_ns <= cooldown_threshold_ns
        })
        .map(|(m, _)| m)
        .collect();

    // Compute run_duration as the filtered window
    let run_duration = if filtered.is_empty() {
        run_start.elapsed().as_secs_f64()
    } else {
        // Find the window from first included request start to last included request end
        // For now, use total elapsed minus warmup/cooldown
        let effective_duration = run_start.elapsed().as_secs_f64() - WARMUP_SECS - COOLDOWN_SECS;
        effective_duration.max(0.1) // avoid division by zero
    };

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
