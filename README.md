# genai-bench-rs

A Rust reimplementation of [genai-bench](https://github.com/sgl-project/genai-bench) for benchmarking LLM serving systems at high concurrency with accurate timing and industry-standard metrics.

## Why Rust?

Python-based benchmark tools (like `benchmark_serving.py` in vLLM) struggle at high concurrency levels. The GIL, asyncio overhead, and garbage collection introduce measurement noise and limit throughput. At 512+ concurrent requests, Python benchmarks often become the bottleneck rather than the server.

**gbrs** solves this:

- **Accurate at high concurrency** (512-1024+) -- Tokio's async runtime handles thousands of concurrent connections with minimal overhead. Timing captures use monotonic `Instant` with nanosecond precision, and first-token timestamps are captured before JSON parsing to avoid deserialization noise.
- **Single binary, zero dependencies** -- `cargo build --release` produces one binary. No Python, no pip, no virtualenvs. Copy it to any machine and run.
- **Low resource footprint** -- ~1KB per concurrent connection. 1024 concurrent requests use ~1MB of memory. No GC pauses during measurement windows.
- **Deterministic prompt generation** -- Generates prompts to exact token counts using a built-in corpus and HuggingFace tokenizer. Different random content per request prevents server-side caching.

## Installation

```bash
git clone <repo-url>
cd genai-bench-rs
cargo build --release

# Option 1: Use the binary directly
./target/release/gbrs benchmark --help

# Option 2: Install to PATH
cargo install --path .
gbrs benchmark --help
```

## Quick Start

### Benchmark a local vLLM/SGLang server

```bash
gbrs benchmark \
    --api-base http://localhost:8080/v1 \
    --model deepseek-ai/DeepSeek-R1-Distill-Qwen-32B \
    --scenario "D(100,100)" \
    --concurrency 64,128,256,512 \
    --duration 2m
```

This runs 4 concurrency levels (64, 128, 256, 512 concurrent requests) for 2 minutes each, using prompts of ~100 input tokens and requesting 100 output tokens.

### Benchmark with multiple scenarios

```bash
gbrs benchmark \
    --api-base http://localhost:8000/v1 \
    --model meta-llama/Llama-3-8B \
    --scenario D(100,100) \
    --scenario D(500,200) \
    --concurrency 1,10,50,100,512 \
    --duration 5m
```

Each scenario runs independently across all concurrency levels. Each level runs for the full duration.

### Benchmark OpenAI or other hosted APIs

```bash
export API_KEY=sk-...

gbrs benchmark \
    --api-base https://api.openai.com/v1 \
    --model gpt-4o-mini \
    --tokenizer Xenova/gpt-4o \
    --scenario D(100,100) \
    --concurrency 1,5,10 \
    --duration 1m \
    --no-ignore-eos
```

Use `--no-ignore-eos` for backends that don't support the `ignore_eos` flag (OpenAI, Anthropic, etc.). Without this flag, the server may stop generating before reaching the target output tokens.

Use `--tokenizer` to specify a different HuggingFace tokenizer when the model name doesn't match a tokenizer on HuggingFace Hub.

### Generate plots from results

```bash
gbrs plot \
    --data ./results/Llama-3-8B_2026-03-14_153042 \
    --percentile p99
```

## CLI Reference

### `gbrs benchmark`

```
Usage: gbrs benchmark [OPTIONS] --api-base <API_BASE> --model <MODEL>
                                --scenario <SCENARIO> --concurrency <CONCURRENCY>

Options:
    --api-base <API_BASE>          OpenAI-compatible API endpoint
    --model <MODEL>                Model name to benchmark
    --tokenizer <TOKENIZER>        HuggingFace tokenizer name (defaults to --model)
    --timeout <TIMEOUT>            Per-request timeout [default: 60s]
    --api-key <API_KEY>            API key (or set $API_KEY env) [env: API_KEY]
    --scenario <SCENARIO>          Scenario spec, repeatable (e.g., D(100,100))
    --concurrency <CONCURRENCY>    Comma-separated concurrency levels
    --duration <DURATION>          Duration per concurrency level [default: 5m]
    --no-ignore-eos                Disable ignore_eos for backends that don't support it
    --output-dir <OUTPUT_DIR>      Output directory [default: ./results/<model>_<datetime>]
```

### `gbrs plot`

```
Usage: gbrs plot [OPTIONS] --data <DATA>

Options:
    --data <DATA>              Path to results directory
    --percentile <PERCENTILE>  Percentile for plots: mean, p50, p90, p99 [default: p99]
    --type <TYPE>              Plot type: all, ttft, ois, latency [default: all]
```

## Scenarios

Scenarios define the input/output token distribution for benchmark requests.

### `D(N,M)` -- Deterministic

Every request targets exactly N input tokens and M output tokens.

```bash
--scenario D(100,100)    # 100 input, 100 output tokens
--scenario D(500,200)    # 500 input, 200 output tokens
--scenario D(7200,1000)  # Long context: 7200 input, 1000 output tokens
```

Prompts are generated from a built-in Shakespeare corpus, randomly shuffled per request to prevent server caching while hitting the exact token count target.

The `ignore_eos` flag (sent by default) forces vLLM/SGLang to generate exactly `max_tokens` output tokens rather than stopping at an EOS token. Use `--no-ignore-eos` for backends that don't support this.

## Metrics

### Per-Request Metrics (5 metrics)

| Metric | Formula | Direction |
|--------|---------|-----------|
| TTFT (Time to First Token) | `first_token - start` | Lower is better |
| TPOT (Time per Output Token) | `(e2e - ttft) / (output_tokens - 1)` | Lower is better |
| E2E Latency | `end - start` | Lower is better |
| Input Throughput | `input_tokens / ttft` | Higher is better |
| Output Throughput (per request) | `1 / tpot` | Higher is better |

### Server-Level Metrics

| Metric | Formula |
|--------|---------|
| Output Throughput of Server (tokens/s) | `sum(output_tokens) / run_duration` |
| Request Throughput (RPS) | `completed_requests / run_duration` |

### Aggregation

Per-request metrics are aggregated with full percentile distributions: min, p1, p5, p10, p25, p50, p75, p90, p95, p99, max, mean, stddev.

**Warmup/cooldown**: Optional ratio-based filtering via `--warmup-ratio` and `--cooldown-ratio` (default: 0, no filtering). For example, `--warmup-ratio 0.1 --cooldown-ratio 0.1` excludes the first 10% and last 10% of requests by completion order.

**Drain filtering**: Requests that complete after the spawn window (i.e., `--duration` expires) are automatically excluded. At high concurrency, the server continues processing in-flight requests after spawning stops, but these run under decreasing concurrency — not true steady-state. Only requests that both started and completed within the active spawning window are included in metrics.

**Server-level `run_duration`**: Uses the configured `--duration` as the measurement window. Only tokens from requests that completed within this window are counted, so server throughput may be slightly conservative at very high concurrency where many requests are still in-flight at cutoff. Increase `--duration` for more accurate results at extreme concurrency levels.

### Percentile Convention

Percentiles in plots are **direction-aware** to always represent "worst case":

| `--percentile` | Latency metrics (lower-is-better) | Throughput per request (higher-is-better) |
|---|---|---|
| `p99` | p99 (worst 1%) | p1 (worst 1%) |
| `p90` | p90 (worst 10%) | p10 (worst 10%) |
| `p50` | p50 (median) | p50 (median) |
| `mean` | mean | mean |

### Edge Cases

- **1 output token**: TPOT and output throughput are undefined (set to null, excluded from aggregation)
- **Output latency < 1ms**: TPOT/throughput unreliable due to timing jitter (set to null)
- **Timed-out requests**: Counted as errors, excluded from all metrics
- **Errors**: Not retried. Counted and reported with breakdown by HTTP status code.

## Output

### Directory Structure

```
results/Llama-3-8B_2026-03-14_153042/
├── D_100_100/
│   ├── summary.csv           # Mean/p99 per concurrency level
│   ├── detailed_stats.csv    # Full percentile distribution
│   └── raw_results.json      # Per-request data
├── D_500_200/
│   ├── summary.csv
│   ├── detailed_stats.csv
│   └── raw_results.json
├── ttft_vs_throughput.svg     # Generated by gbrs plot
├── ois_vs_throughput.svg
└── e2e_latency_vs_rps.svg
```

### summary.csv

One row per concurrency level with key metrics:

```
concurrency,ttft_mean,ttft_p99,tpot_mean,tpot_p99,e2e_latency_mean,e2e_latency_p99,output_throughput_mean,output_throughput_server,rps,total_requests,error_count,error_rate
1,0.150000,0.250000,0.015000,0.025000,1.660000,2.100000,66.00,60.20,0.58,35,0,0.0000
10,0.220000,0.480000,0.018000,0.030000,1.950000,2.800000,55.20,466.70,5.12,310,2,0.0060
```

### detailed_stats.csv

Full distribution per concurrency level, per metric:

```
concurrency,metric,min,p1,p5,p10,p25,p50,p75,p90,p95,p99,max,mean,stddev
1,ttft_s,0.120000,0.120000,0.130000,...
1,tpot_s,...
```

### raw_results.json

Complete per-request data for custom analysis:

```json
{
  "version": "1.0",
  "metadata": {
    "api_base": "http://localhost:8000/v1",
    "model": "meta-llama/Llama-3-8B",
    "scenario": "D(100,100)"
  },
  "results": [
    {
      "concurrency": 10,
      "run_duration_s": 60.0,
      "output_throughput_server_tps": 466.7,
      "requests": [
        {
          "request_id": 0,
          "ttft_s": 0.152,
          "tpot_s": 0.01527,
          "e2e_latency_s": 1.662,
          "num_input_tokens": 103,
          "num_output_tokens": 100,
          "error": null
        }
      ]
    }
  ]
}
```

### Terminal Output

During the run, gbrs displays a progress bar and accumulating summary table:

```
genai-bench-rs — D(100,100) — model: meta-llama/Llama-3-8B

Completed runs:
┌─────────────┬──────────┬──────────┬─────────┬───────┬───────┬──────┬────────┐
│ Concurrency │ TTFT p99 │ TPOT p99 │ E2E p99 │ OT/req│ OT/srv│ RPS  │ Errors │
├─────────────┼──────────┼──────────┼─────────┼───────┼───────┼──────┼────────┤
│ 1           │ 0.250    │ 0.0250   │ 2.100   │ 66.0  │ 60.2  │ 0.58 │ 0      │
│ 10          │ 0.480    │ 0.0300   │ 2.800   │ 55.2  │ 466.7 │ 5.12 │ 2      │
└─────────────┴──────────┴──────────┴─────────┴───────┴───────┴──────┴────────┘

Concurrency 50 [████████████████████░░░░░░░░░░░░░░░░░░░░] 234/500 reqs [38s left]
```

Errors are logged in real-time and summarized at the end:

```
[WARN] Request 342: HTTP 429 Rate limited

Errors:
┌─────────────┬───────┬─────────────────────────────┐
│ Concurrency │ Count │ Breakdown                   │
├─────────────┼───────┼─────────────────────────────┤
│ 100         │ 15    │ 429: 12, 503: 3             │
└─────────────┴───────┴─────────────────────────────┘
```

### Plots

Three SVG charts are generated by `gbrs plot`:

1. **TTFT vs Output Throughput of Server** -- How prefill latency degrades as server load increases
2. **Output Throughput per Request vs Output Throughput of Server** -- How per-request generation speed degrades under load
3. **E2E Latency vs RPS** -- Overall latency vs request throughput

Each scenario appears as a separate line with a legend. Data points represent concurrency levels.

## Sample Results

Benchmark of **DeepSeek-R1-Distill-Qwen-32B** on 2x NVIDIA H100 GPUs, scenario D(100,100), 1 minute per concurrency level.

### HTTP (SGLang)

| Concurrency | TTFT p99 | TPOT p99 | E2E p99 | OT/req | OT/srv  | RPS   |
|-------------|----------|----------|---------|--------|---------|-------|
| 64          | 0.605    | 0.0202   | 2.416   | 56.1   | 3034.8  | 30.35 |
| 128         | 1.099    | 0.0243   | 2.989   | 49.3   | 4728.4  | 47.28 |
| 256         | 1.561    | 0.0366   | 4.282   | 32.9   | 6478.7  | 64.79 |
| 512         | 3.180    | 0.0676   | 7.810   | 18.9   | **7047.7** | 70.48 |
| 1024        | 11.348   | 0.0922   | 19.081  | 14.4   | 6984.3  | 69.84 |
| 2048        | 21.741   | 0.1882   | 33.308  | 13.8   | 5700.4  | 57.00 |
| 4096        | 48.233   | 0.0925   | 54.441  | 13.6   | 4261.0  | 42.61 |

### gRPC + SMG Router (SGLang)

| Concurrency | TTFT p99 | TPOT p99 | E2E p99 | OT/req | OT/srv  | RPS   |
|-------------|----------|----------|---------|--------|---------|-------|
| 64          | 0.408    | 0.0187   | 1.997   | 61.2   | 3220.7  | 32.21 |
| 128         | 0.740    | 0.0222   | 2.781   | 51.2   | 5011.7  | 50.12 |
| 256         | 1.444    | 0.0356   | 3.967   | 36.1   | 6614.8  | 66.15 |
| 512         | 2.692    | 0.0629   | 7.010   | 20.0   | **7559.5** | 75.59 |
| 1024        | 10.938   | 0.0944   | 19.081  | 14.5   | 7033.4  | 70.33 |
| 2048        | 21.140   | 0.0919   | 27.710  | 14.0   | 5950.8  | 59.51 |
| 4096        | 47.644   | 0.1909   | 59.610  | 13.9   | 3851.9  | 38.52 |

### Analysis

**Peak throughput**: Both backends peak at concurrency 512 -- gRPC achieves **7559 tok/s** vs HTTP's **7048 tok/s** (7.3% higher).

**Latency at low concurrency**: gRPC shows consistently lower TTFT across all levels. At concurrency 64, gRPC TTFT p99 is 0.408s vs HTTP's 0.605s (33% lower). This reflects reduced connection overhead with gRPC's persistent HTTP/2 streams.

**Per-request throughput**: gRPC maintains better per-request output speed (OT/req). At concurrency 512, gRPC delivers 20.0 tok/s per request vs HTTP's 18.9 tok/s.

**Degradation at high concurrency**: Both backends degrade sharply beyond 512 concurrent requests. At 4096, server throughput drops to ~55% of peak (HTTP: 4261, gRPC: 3852) while TTFT explodes to 48s+. This indicates the server's scheduling and memory become the bottleneck, not the transport layer.

**Key takeaway**: gRPC provides a modest but consistent advantage at every concurrency level, with the gap most visible in TTFT. Both backends saturate around 512 concurrent requests for this model/GPU configuration. Running beyond 1024 concurrency yields no throughput benefit and dramatically hurts latency.

## Examples

### Quick smoke test

```bash
gbrs benchmark \
    --api-base http://localhost:8000/v1 \
    --model meta-llama/Llama-3-8B \
    --scenario D(10,10) \
    --concurrency 1,2 \
    --duration 1m
```

### Full benchmark sweep

```bash
gbrs benchmark \
    --api-base http://localhost:8000/v1 \
    --model meta-llama/Llama-3-8B \
    --scenario D(100,100) \
    --scenario D(500,200) \
    --scenario D(2000,500) \
    --concurrency 1,2,4,8,16,32,64,128,256,512 \
    --duration 5m

# Generate plots
gbrs plot --data ./results/Llama-3-8B_* --percentile p99
```

### Compare two models

Run benchmarks separately, then compare the output CSVs or overlay plots.

```bash
gbrs benchmark --api-base http://server1:8000/v1 --model model-A \
    --scenario D(100,100) --concurrency 1,10,50,100 --duration 2m \
    --output-dir ./results/model-A

gbrs benchmark --api-base http://server2:8000/v1 --model model-B \
    --scenario D(100,100) --concurrency 1,10,50,100 --duration 2m \
    --output-dir ./results/model-B
```

### High concurrency stress test

```bash
# Increase file descriptor limit first
ulimit -n 65536

gbrs benchmark \
    --api-base http://localhost:8000/v1 \
    --model meta-llama/Llama-3-8B \
    --scenario D(100,100) \
    --concurrency 64,128,256,512,1024,2048,4096 \
    --duration 5m \
    --timeout 120s
```

gbrs will warn if the file descriptor limit is too low for the requested concurrency.

## How It Works

1. **Prompt generation**: For each request, shuffles lines from a built-in Shakespeare corpus and accumulates them until reaching the target input token count, using the HuggingFace tokenizer for accurate counting.

2. **Request execution**: Sends streaming POST to `/v1/chat/completions` with `stream=true` and `ignore_eos=true`. Captures three timestamps (all relative to benchmark run start) using monotonic `Instant`:
   - `start_ns`: when the request is initiated
   - `first_token_ns`: when the first non-empty content SSE chunk arrives (captured before JSON parsing)
   - `end_ns`: when the `[DONE]` marker is received

3. **Concurrency control**: A Tokio semaphore maintains exactly N requests in-flight. When one completes, the next starts immediately, maintaining constant pressure on the server.

4. **Warmup/cooldown**: Requests initiated in the first or last 5 seconds of each concurrency level are excluded from all metrics. Server-level metrics (throughput, RPS) use the filtered window duration.

5. **Metric computation**: Five per-request metrics are derived from three timestamps and two token counts (from the server's `usage` field). These are aggregated into full percentile distributions.

6. **Graceful shutdown**: Ctrl+C stops spawning new requests, waits for in-flight requests to complete, computes metrics for completed requests, and saves partial results.

## Requirements

- Rust 1.70+ (for building)
- An OpenAI-compatible API endpoint (vLLM, SGLang, OpenAI, etc.)
- Internet access during first build (to download the HuggingFace tokenizer)

## License

MIT
