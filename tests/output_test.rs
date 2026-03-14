use genai_bench_rs::metrics::*;
use genai_bench_rs::output::*;

#[test]
fn test_summary_csv_headers() {
    let headers = summary_csv_headers();
    assert!(headers.contains(&"concurrency"));
    assert!(headers.contains(&"ttft_mean"));
    assert!(headers.contains(&"ttft_p99"));
    assert!(headers.contains(&"output_throughput_server"));
    assert!(headers.contains(&"rps"));
    assert!(headers.contains(&"error_count"));
}

#[test]
fn test_summary_csv_row() {
    let agg = make_test_aggregated_metrics();
    let row = summary_csv_row(&agg);
    assert_eq!(row[0], "10"); // concurrency
    assert!(!row[1].is_empty()); // ttft_mean
}

fn make_test_aggregated_metrics() -> AggregatedMetrics {
    let dist = DistributionStats {
        min: 0.1, p1: 0.1, p5: 0.11, p10: 0.12, p25: 0.13,
        p50: 0.15, p75: 0.18, p90: 0.20, p95: 0.22, p99: 0.25,
        max: 0.3, mean: 0.15, stddev: 0.03,
    };
    AggregatedMetrics {
        concurrency: 10,
        run_duration_s: 60.0,
        output_throughput_server_tps: 466.7,
        rps: 5.12,
        total_requests: 310,
        error_count: 2,
        error_rate: 0.006,
        stats: AggregatedStats {
            ttft: dist.clone(),
            tpot: dist.clone(),
            e2e_latency: dist.clone(),
            input_throughput: dist.clone(),
            output_throughput: dist,
        },
    }
}
