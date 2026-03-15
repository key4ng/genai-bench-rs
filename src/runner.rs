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

pub struct RunConfig {
    pub duration: Duration,
    pub concurrency: u32,
    pub warmup_ratio: f64,
    pub cooldown_ratio: f64,
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
    let producer_pb = pb.clone();
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
            let pb_clone = producer_pb.clone();
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
                        pb_clone.println(msg);
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

    // Separate successes and errors, preserving completion order
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

    // Step 1: Ratio-based warmup/cooldown filter.
    // Skip first N% and last N% of successful requests by completion order.
    let total_success = successes.len();
    let warmup_count = (total_success as f64 * config.warmup_ratio) as usize;
    let cooldown_count = (total_success as f64 * config.cooldown_ratio) as usize;
    let end_idx = total_success.saturating_sub(cooldown_count);

    let after_ratio_filter: Vec<(RequestMetrics, u64, u64)> =
        if warmup_count + cooldown_count < total_success {
            successes.drain(warmup_count..end_idx).collect()
        } else {
            successes
        };

    // Step 2: Drain filter — exclude requests that completed after spawning stopped.
    // These ran partially under decreasing concurrency, not true steady-state.
    let duration_ns = config.duration.as_nanos() as u64;
    let before_drain = after_ratio_filter.len();
    let filtered_with_times: Vec<(RequestMetrics, u64, u64)> = after_ratio_filter
        .into_iter()
        .filter(|(_, _, e_ns)| *e_ns <= duration_ns)
        .collect();
    let drain_excluded = before_drain - filtered_with_times.len();

    if warmup_count + cooldown_count + drain_excluded > 0 && !filtered_with_times.is_empty() {
        let mut parts = Vec::new();
        if warmup_count > 0 {
            parts.push(format!("warmup: {}", warmup_count));
        }
        if cooldown_count > 0 {
            parts.push(format!("cooldown: {}", cooldown_count));
        }
        if drain_excluded > 0 {
            parts.push(format!("drain: {}", drain_excluded));
        }
        eprintln!(
            "Filtered {}/{} requests ({})",
            filtered_with_times.len(),
            total_success,
            parts.join(", ")
        );
    }

    // run_duration = configured duration (the active spawning window).
    // Only completed requests within this window are counted, so server
    // throughput may be slightly conservative at high concurrency where
    // many requests are still in-flight at cutoff. Increase --duration
    // for more accurate results.
    let run_duration = config.duration.as_secs_f64();

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
