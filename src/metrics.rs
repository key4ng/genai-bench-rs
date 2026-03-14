use serde::Serialize;

#[derive(Debug, Clone)]
pub struct RawRequestResult {
    pub request_id: u64,
    pub start_time_ns: u64,
    pub first_token_time_ns: u64,
    pub end_time_ns: u64,
    pub num_input_tokens: u32,
    pub num_output_tokens: u32,
    pub reasoning_tokens: u32,
    pub error: Option<RequestError>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RequestError {
    pub code: u16,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RequestMetrics {
    pub request_id: u64,
    pub ttft_s: f64,
    pub e2e_latency_s: f64,
    pub tpot_s: Option<f64>,
    pub output_throughput_tps: Option<f64>,
    pub input_throughput_tps: Option<f64>,
    pub num_input_tokens: u32,
    pub num_output_tokens: u32,
    pub reasoning_tokens: u32,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct DistributionStats {
    pub min: f64,
    pub p1: f64,
    pub p5: f64,
    pub p10: f64,
    pub p25: f64,
    pub p50: f64,
    pub p75: f64,
    pub p90: f64,
    pub p95: f64,
    pub p99: f64,
    pub max: f64,
    pub mean: f64,
    pub stddev: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct AggregatedStats {
    pub ttft: DistributionStats,
    pub tpot: DistributionStats,
    pub e2e_latency: DistributionStats,
    pub input_throughput: DistributionStats,
    pub output_throughput: DistributionStats,
}

#[derive(Debug, Clone, Serialize)]
pub struct AggregatedMetrics {
    pub concurrency: u32,
    pub run_duration_s: f64,
    pub output_throughput_server_tps: f64,
    pub rps: f64,
    pub total_requests: usize,
    pub error_count: usize,
    pub error_rate: f64,
    pub stats: AggregatedStats,
}

impl AggregatedMetrics {
    pub fn empty() -> Self {
        let empty_dist = DistributionStats {
            min: 0.0,
            p1: 0.0,
            p5: 0.0,
            p10: 0.0,
            p25: 0.0,
            p50: 0.0,
            p75: 0.0,
            p90: 0.0,
            p95: 0.0,
            p99: 0.0,
            max: 0.0,
            mean: 0.0,
            stddev: 0.0,
        };
        Self {
            concurrency: 0,
            run_duration_s: 0.0,
            output_throughput_server_tps: 0.0,
            rps: 0.0,
            total_requests: 0,
            error_count: 0,
            error_rate: 0.0,
            stats: AggregatedStats {
                ttft: empty_dist.clone(),
                tpot: empty_dist.clone(),
                e2e_latency: empty_dist.clone(),
                input_throughput: empty_dist.clone(),
                output_throughput: empty_dist,
            },
        }
    }
}

pub fn compute_request_metrics(raw: &RawRequestResult) -> Option<RequestMetrics> {
    if raw.error.is_some() {
        return None;
    }

    let ttft_s = (raw.first_token_time_ns - raw.start_time_ns) as f64 / 1_000_000_000.0;
    let e2e_latency_s = (raw.end_time_ns - raw.start_time_ns) as f64 / 1_000_000_000.0;
    let output_latency_s = e2e_latency_s - ttft_s;

    let input_throughput_tps = if ttft_s > 0.0 {
        Some(raw.num_input_tokens as f64 / ttft_s)
    } else {
        None
    };

    let (tpot_s, output_throughput_tps) = if raw.num_output_tokens > 1 && output_latency_s >= 0.001
    {
        let tpot = output_latency_s / (raw.num_output_tokens as f64 - 1.0);
        (Some(tpot), Some(1.0 / tpot))
    } else {
        (None, None)
    };

    Some(RequestMetrics {
        request_id: raw.request_id,
        ttft_s,
        e2e_latency_s,
        tpot_s,
        output_throughput_tps,
        input_throughput_tps,
        num_input_tokens: raw.num_input_tokens,
        num_output_tokens: raw.num_output_tokens,
        reasoning_tokens: raw.reasoning_tokens,
    })
}

pub fn compute_distribution(values: &[f64]) -> DistributionStats {
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());

    let n = sorted.len();
    let mean = sorted.iter().sum::<f64>() / n as f64;
    let variance = sorted.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / n as f64;

    DistributionStats {
        min: sorted[0],
        p1: percentile_sorted(&sorted, 1.0),
        p5: percentile_sorted(&sorted, 5.0),
        p10: percentile_sorted(&sorted, 10.0),
        p25: percentile_sorted(&sorted, 25.0),
        p50: percentile_sorted(&sorted, 50.0),
        p75: percentile_sorted(&sorted, 75.0),
        p90: percentile_sorted(&sorted, 90.0),
        p95: percentile_sorted(&sorted, 95.0),
        p99: percentile_sorted(&sorted, 99.0),
        max: sorted[n - 1],
        mean,
        stddev: variance.sqrt(),
    }
}

fn percentile_sorted(sorted: &[f64], p: f64) -> f64 {
    if sorted.len() == 1 {
        return sorted[0];
    }
    let rank = (p / 100.0) * (sorted.len() - 1) as f64;
    let lower = rank.floor() as usize;
    let upper = rank.ceil() as usize;
    let frac = rank - lower as f64;
    sorted[lower] * (1.0 - frac) + sorted[upper] * frac
}

pub fn aggregate_metrics(metrics: &[RequestMetrics], run_duration_s: f64) -> AggregatedMetrics {
    let ttft_values: Vec<f64> = metrics.iter().map(|m| m.ttft_s).collect();
    let e2e_values: Vec<f64> = metrics.iter().map(|m| m.e2e_latency_s).collect();
    let tpot_values: Vec<f64> = metrics.iter().filter_map(|m| m.tpot_s).collect();
    let ot_values: Vec<f64> = metrics
        .iter()
        .filter_map(|m| m.output_throughput_tps)
        .collect();
    let it_values: Vec<f64> = metrics
        .iter()
        .filter_map(|m| m.input_throughput_tps)
        .collect();

    let total_output_tokens: u64 = metrics.iter().map(|m| m.num_output_tokens as u64).sum();

    AggregatedMetrics {
        concurrency: 0, // Set by caller
        run_duration_s,
        output_throughput_server_tps: total_output_tokens as f64 / run_duration_s,
        rps: metrics.len() as f64 / run_duration_s,
        total_requests: metrics.len(),
        error_count: 0, // Set by caller
        error_rate: 0.0,
        stats: AggregatedStats {
            ttft: compute_distribution(&ttft_values),
            tpot: compute_distribution(&tpot_values),
            e2e_latency: compute_distribution(&e2e_values),
            input_throughput: compute_distribution(&it_values),
            output_throughput: compute_distribution(&ot_values),
        },
    }
}
