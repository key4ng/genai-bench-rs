use genai_bench_rs::metrics::*;

#[test]
fn test_compute_request_metrics_normal() {
    let raw = RawRequestResult {
        request_id: 0,
        start_time_ns: 0,
        first_token_time_ns: 150_000_000, // 0.15s
        end_time_ns: 1_650_000_000,       // 1.65s
        num_input_tokens: 100,
        num_output_tokens: 100,
        reasoning_tokens: 0,
        run_offset_ns: 0,
        generated_text: String::new(),
        error: None,
    };

    let metrics = compute_request_metrics(&raw).unwrap();
    assert!((metrics.ttft_s - 0.15).abs() < 0.001);
    assert!((metrics.e2e_latency_s - 1.65).abs() < 0.001);
    // tpot = (1.65 - 0.15) / (100 - 1) = 1.5 / 99
    assert!((metrics.tpot_s.unwrap() - 0.01515).abs() < 0.001);
    // output_throughput = 1 / tpot
    assert!((metrics.output_throughput_tps.unwrap() - 66.0).abs() < 1.0);
    // input_throughput = 100 / 0.15
    assert!((metrics.input_throughput_tps.unwrap() - 666.67).abs() < 1.0);
}

#[test]
fn test_compute_request_metrics_single_output_token() {
    let raw = RawRequestResult {
        request_id: 0,
        start_time_ns: 0,
        first_token_time_ns: 150_000_000,
        end_time_ns: 300_000_000,
        num_input_tokens: 100,
        num_output_tokens: 1,
        reasoning_tokens: 0,
        run_offset_ns: 0,
        generated_text: String::new(),
        error: None,
    };

    let metrics = compute_request_metrics(&raw).unwrap();
    assert!(metrics.tpot_s.is_none());
    assert!(metrics.output_throughput_tps.is_none());
}

#[test]
fn test_compute_request_metrics_sub_ms_output_latency() {
    let raw = RawRequestResult {
        request_id: 0,
        start_time_ns: 0,
        first_token_time_ns: 150_000_000,
        end_time_ns: 150_500_000, // output_latency = 0.0005s
        num_input_tokens: 100,
        num_output_tokens: 10,
        reasoning_tokens: 0,
        run_offset_ns: 0,
        generated_text: String::new(),
        error: None,
    };

    let metrics = compute_request_metrics(&raw).unwrap();
    // Sub-ms output latency → tpot/throughput unreliable
    assert!(metrics.tpot_s.is_none());
    assert!(metrics.output_throughput_tps.is_none());
}

#[test]
fn test_compute_request_metrics_error() {
    let raw = RawRequestResult {
        request_id: 0,
        start_time_ns: 0,
        first_token_time_ns: 0,
        end_time_ns: 0,
        num_input_tokens: 0,
        num_output_tokens: 0,
        reasoning_tokens: 0,
        run_offset_ns: 0,
        generated_text: String::new(),
        error: Some(RequestError {
            code: 429,
            message: "Rate limited".into(),
        }),
    };

    let result = compute_request_metrics(&raw);
    assert!(result.is_none());
}

#[test]
fn test_aggregate_metrics_basic() {
    let metrics = vec![
        RequestMetrics {
            request_id: 0,
            ttft_s: 0.10,
            e2e_latency_s: 1.5,
            tpot_s: Some(0.015),
            output_throughput_tps: Some(66.7),
            input_throughput_tps: Some(666.7),
            num_input_tokens: 100,
            num_output_tokens: 100,
            reasoning_tokens: 0,
        },
        RequestMetrics {
            request_id: 1,
            ttft_s: 0.20,
            e2e_latency_s: 2.0,
            tpot_s: Some(0.020),
            output_throughput_tps: Some(50.0),
            input_throughput_tps: Some(500.0),
            num_input_tokens: 100,
            num_output_tokens: 100,
            reasoning_tokens: 0,
        },
    ];

    let agg = aggregate_metrics(&metrics, 3.5);
    assert_eq!(agg.total_requests, 2);
    assert!((agg.stats.ttft.mean - 0.15).abs() < 0.001);
    assert!((agg.output_throughput_server_tps - 200.0 / 3.5).abs() < 1.0);
    assert!((agg.rps - 2.0 / 3.5).abs() < 0.01);
}

#[test]
fn test_percentile_calculation() {
    let values: Vec<f64> = (1..=100).map(|x| x as f64).collect();
    let stats = compute_distribution(&values);
    assert!((stats.p50 - 50.0).abs() < 1.0);
    assert!((stats.p99 - 99.0).abs() < 1.0);
    assert!((stats.p1 - 1.0).abs() < 1.0);
    assert!((stats.mean - 50.5).abs() < 0.1);
    assert_eq!(stats.min, 1.0);
    assert_eq!(stats.max, 100.0);
}
