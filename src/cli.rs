use clap::{Parser, Subcommand};
use std::time::Duration;

#[derive(Parser)]
#[command(name = "gbrs", about = "Lightweight LLM serving benchmark tool")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Run a benchmark against an LLM serving endpoint
    Benchmark(BenchmarkArgs),
    /// Generate plots from benchmark results
    Plot(PlotArgs),
}

#[derive(Parser)]
pub struct BenchmarkArgs {
    /// OpenAI-compatible API endpoint
    #[arg(long)]
    pub api_base: String,

    /// Model name to benchmark
    #[arg(long)]
    pub model: String,

    /// HuggingFace tokenizer name or path (defaults to --model)
    #[arg(long)]
    pub tokenizer: Option<String>,

    /// Per-request timeout (e.g., 60s, 2m)
    #[arg(long, default_value = "60s", value_parser = parse_duration)]
    pub timeout: Duration,

    /// API authentication key (defaults to $API_KEY env)
    #[arg(long, env = "API_KEY")]
    pub api_key: Option<String>,

    /// Scenario spec, repeatable (e.g., D(100,100))
    #[arg(long, required = true)]
    pub scenario: Vec<String>,

    /// Comma-separated concurrency levels (e.g., 1,10,50,100)
    #[arg(long, value_delimiter = ',', required = true)]
    pub concurrency: Vec<u32>,

    /// Duration per concurrency level (e.g., 60s, 5m, 1h)
    #[arg(long, default_value = "5m", value_parser = parse_duration)]
    pub duration: Duration,

    /// Ratio of initial requests to exclude as warmup (0.0-1.0)
    #[arg(long, default_value_t = 0.0)]
    pub warmup_ratio: f64,

    /// Ratio of final requests to exclude as cooldown (0.0-1.0)
    #[arg(long, default_value_t = 0.0)]
    pub cooldown_ratio: f64,

    /// Disable ignore_eos (for backends like OpenAI that don't support it)
    #[arg(long, default_value_t = false)]
    pub no_ignore_eos: bool,

    /// Output directory (default: ./results/<model>_<datetime>)
    #[arg(long)]
    pub output_dir: Option<String>,
}

#[derive(Parser)]
pub struct PlotArgs {
    /// Path to results directory
    #[arg(long)]
    pub data: String,

    /// Percentile for plots (mean, p50, p90, p99)
    #[arg(long, default_value = "p99")]
    pub percentile: String,

    /// Plot type (all, ttft, ois, latency)
    #[arg(long, default_value = "all")]
    pub r#type: String,
}

/// Parse duration strings like "60s", "5m", "1h"
pub fn parse_duration(s: &str) -> Result<Duration, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("duration cannot be empty".to_string());
    }

    let (num_str, unit) = if let Some(n) = s.strip_suffix('s') {
        (n, "s")
    } else if let Some(n) = s.strip_suffix('m') {
        (n, "m")
    } else if let Some(n) = s.strip_suffix('h') {
        (n, "h")
    } else {
        return Err(format!(
            "invalid duration '{}': must end with s, m, or h",
            s
        ));
    };

    let num: u64 = num_str
        .parse()
        .map_err(|_| format!("invalid number in duration: '{}'", num_str))?;

    match unit {
        "s" => Ok(Duration::from_secs(num)),
        "m" => Ok(Duration::from_secs(num * 60)),
        "h" => Ok(Duration::from_secs(num * 3600)),
        _ => unreachable!(),
    }
}

impl BenchmarkArgs {
    pub fn tokenizer_name(&self) -> &str {
        self.tokenizer.as_deref().unwrap_or(&self.model)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.duration < Duration::from_secs(60) {
            return Err("duration must be at least 1m for reliable metrics".to_string());
        }
        if self.warmup_ratio + self.cooldown_ratio >= 1.0 {
            return Err(format!(
                "warmup_ratio ({}) + cooldown_ratio ({}) must be less than 1.0",
                self.warmup_ratio, self.cooldown_ratio
            ));
        }
        Ok(())
    }

    pub fn output_dir(&self) -> String {
        if let Some(ref dir) = self.output_dir {
            dir.clone()
        } else {
            let model_clean = self.model.replace('/', "-");
            let now = chrono::Local::now().format("%Y-%m-%d_%H%M%S");
            format!("./results/{}_{}", model_clean, now)
        }
    }
}
