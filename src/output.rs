use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::Path;

use comfy_table::{Cell, Color, Table};

use crate::metrics::{AggregatedMetrics, RawRequestResult};

/// (concurrency, requests, run_duration_s, output_throughput_server_tps, total_requests, error_count)
pub type RawJsonEntry<'a> = (u32, &'a [RawRequestResult], f64, f64, usize, usize);

pub fn summary_csv_headers() -> Vec<&'static str> {
    vec![
        "concurrency",
        "ttft_mean",
        "ttft_p99",
        "tpot_mean",
        "tpot_p99",
        "e2e_latency_mean",
        "e2e_latency_p99",
        "output_throughput_mean",
        "output_throughput_server",
        "rps",
        "total_requests",
        "error_count",
        "error_rate",
    ]
}

pub fn summary_csv_row(agg: &AggregatedMetrics) -> Vec<String> {
    vec![
        agg.concurrency.to_string(),
        format!("{:.6}", agg.stats.ttft.mean),
        format!("{:.6}", agg.stats.ttft.p99),
        format!("{:.6}", agg.stats.tpot.mean),
        format!("{:.6}", agg.stats.tpot.p99),
        format!("{:.6}", agg.stats.e2e_latency.mean),
        format!("{:.6}", agg.stats.e2e_latency.p99),
        format!("{:.2}", agg.stats.output_throughput.mean),
        format!("{:.2}", agg.output_throughput_server_tps),
        format!("{:.2}", agg.rps),
        agg.total_requests.to_string(),
        agg.error_count.to_string(),
        format!("{:.4}", agg.error_rate),
    ]
}

pub fn write_summary_csv(path: &Path, results: &[AggregatedMetrics]) -> anyhow::Result<()> {
    let mut wtr = csv::Writer::from_path(path)?;
    wtr.write_record(summary_csv_headers())?;
    for agg in results {
        wtr.write_record(summary_csv_row(agg))?;
    }
    wtr.flush()?;
    Ok(())
}

pub fn write_detailed_stats_csv(path: &Path, results: &[AggregatedMetrics]) -> anyhow::Result<()> {
    let mut wtr = csv::Writer::from_path(path)?;
    wtr.write_record([
        "concurrency",
        "metric",
        "min",
        "p1",
        "p5",
        "p10",
        "p25",
        "p50",
        "p75",
        "p90",
        "p95",
        "p99",
        "max",
        "mean",
        "stddev",
    ])?;

    for agg in results {
        let metrics_map = [
            ("ttft_s", &agg.stats.ttft),
            ("tpot_s", &agg.stats.tpot),
            ("e2e_latency_s", &agg.stats.e2e_latency),
            ("input_throughput_tps", &agg.stats.input_throughput),
            ("output_throughput_tps", &agg.stats.output_throughput),
        ];

        for (name, dist) in &metrics_map {
            wtr.write_record([
                &agg.concurrency.to_string(),
                &name.to_string(),
                &format!("{:.6}", dist.min),
                &format!("{:.6}", dist.p1),
                &format!("{:.6}", dist.p5),
                &format!("{:.6}", dist.p10),
                &format!("{:.6}", dist.p25),
                &format!("{:.6}", dist.p50),
                &format!("{:.6}", dist.p75),
                &format!("{:.6}", dist.p90),
                &format!("{:.6}", dist.p95),
                &format!("{:.6}", dist.p99),
                &format!("{:.6}", dist.max),
                &format!("{:.6}", dist.mean),
                &format!("{:.6}", dist.stddev),
            ])?;
        }
    }

    wtr.flush()?;
    Ok(())
}

pub fn write_raw_json(
    path: &Path,
    metadata: &serde_json::Value,
    results: &[RawJsonEntry<'_>],
) -> anyhow::Result<()> {
    use crate::metrics::compute_request_metrics;

    let json = serde_json::json!({
        "version": "1.0",
        "metadata": metadata,
        "results": results.iter().map(|(conc, reqs, dur, ot_server, total, errors)| {
            let requests: Vec<serde_json::Value> = reqs.iter().map(|raw| {
                if let Some(ref err) = raw.error {
                    serde_json::json!({
                        "request_id": raw.request_id,
                        "error": { "code": err.code, "message": &err.message }
                    })
                } else if let Some(m) = compute_request_metrics(raw) {
                    serde_json::json!({
                        "request_id": raw.request_id,
                        "num_input_tokens": m.num_input_tokens,
                        "num_output_tokens": m.num_output_tokens,
                        "reasoning_tokens": m.reasoning_tokens,
                        "ttft_s": m.ttft_s,
                        "tpot_s": m.tpot_s,
                        "e2e_latency_s": m.e2e_latency_s,
                        "input_throughput_tps": m.input_throughput_tps,
                        "output_throughput_tps": m.output_throughput_tps,
                        "error": null
                    })
                } else {
                    serde_json::json!({
                        "request_id": raw.request_id,
                        "error": null
                    })
                }
            }).collect();

            serde_json::json!({
                "concurrency": conc,
                "run_duration_s": dur,
                "output_throughput_server_tps": ot_server,
                "total_requests": total,
                "error_count": errors,
                "requests": requests,
            })
        }).collect::<Vec<_>>(),
    });

    let mut f = fs::File::create(path)?;
    f.write_all(serde_json::to_string_pretty(&json)?.as_bytes())?;
    Ok(())
}

/// Track how many lines the previous summary table used, so we can overwrite it.
static PREV_TABLE_LINES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

pub fn reset_summary_table() {
    PREV_TABLE_LINES.store(0, std::sync::atomic::Ordering::Relaxed);
}

pub fn print_summary_table(results: &[AggregatedMetrics]) {
    let mut table = Table::new();
    table.set_header(vec![
        "Concurrency",
        "TTFT p99",
        "TPOT p99",
        "E2E p99",
        "OT/req",
        "OT/srv",
        "RPS",
        "Errors",
    ]);

    for agg in results {
        let error_cell = if agg.error_count > 0 {
            Cell::new(agg.error_count).fg(Color::Red)
        } else {
            Cell::new(agg.error_count).fg(Color::Green)
        };
        table.add_row(vec![
            Cell::new(agg.concurrency).fg(Color::Cyan),
            Cell::new(format!("{:.3}", agg.stats.ttft.p99)),
            Cell::new(format!("{:.4}", agg.stats.tpot.p99)),
            Cell::new(format!("{:.3}", agg.stats.e2e_latency.p99)),
            Cell::new(format!("{:.1}", agg.stats.output_throughput.mean)).fg(Color::Yellow),
            Cell::new(format!("{:.1}", agg.output_throughput_server_tps)).fg(Color::Yellow),
            Cell::new(format!("{:.2}", agg.rps)).fg(Color::Yellow),
            error_cell,
        ]);
    }

    let output = format!("\n\x1b[1mCompleted runs:\x1b[0m\n{table}");
    let line_count = output.lines().count();

    // Move cursor up to overwrite previous table
    let prev = PREV_TABLE_LINES.load(std::sync::atomic::Ordering::Relaxed);
    if prev > 0 {
        // Move up prev lines and clear each
        eprint!("\x1b[{}A", prev);
        for _ in 0..prev {
            eprintln!("\x1b[2K");
        }
        eprint!("\x1b[{}A", prev);
    }

    PREV_TABLE_LINES.store(line_count, std::sync::atomic::Ordering::Relaxed);
    eprintln!("{output}");
}

pub fn print_error_summary(error_data: &[(u32, HashMap<String, usize>)]) {
    let has_errors = error_data.iter().any(|(_, m)| !m.is_empty());
    if !has_errors {
        return;
    }

    let mut table = Table::new();
    table.set_header(vec!["Concurrency", "Count", "Breakdown"]);

    for (concurrency, breakdown) in error_data {
        if breakdown.is_empty() {
            continue;
        }
        let count: usize = breakdown.values().sum();
        let details: Vec<String> = breakdown
            .iter()
            .map(|(k, v)| format!("{}: {}", k, v))
            .collect();
        table.add_row(vec![
            Cell::new(concurrency),
            Cell::new(count),
            Cell::new(details.join(", ")),
        ]);
    }

    println!("\nErrors:");
    println!("{table}");
}
