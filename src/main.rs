mod cli;
mod client;
mod metrics;
mod output;
mod plot;
mod runner;
mod scenario;
mod tokenizer;

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::Result;
use clap::Parser;

use cli::{Cli, Commands};
use client::BenchmarkClient;
use output::{
    print_error_summary, print_summary_table, write_detailed_stats_csv, write_raw_json,
    write_summary_csv, RawJsonEntry,
};
use plot::{generate_plots, PlotConfig, PlotType};
use runner::{run_benchmark, RunConfig};
use scenario::parse_scenario;
use tokenizer::PromptGenerator;

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Benchmark(args) => run_benchmark_command(args).await,
        Commands::Plot(args) => run_plot_command(args),
    }
}

async fn run_benchmark_command(args: cli::BenchmarkArgs) -> Result<()> {
    // Check fd limits
    #[cfg(unix)]
    {
        let max_conc = *args.concurrency.iter().max().unwrap_or(&1);
        check_fd_limit(max_conc);
    }

    // Parse scenarios
    let scenarios: Vec<_> = args
        .scenario
        .iter()
        .map(|s| parse_scenario(s))
        .collect::<Result<Vec<_>>>()?;

    // Load tokenizer
    eprintln!("Loading tokenizer: {}", args.tokenizer_name());
    let prompt_generator = Arc::new(PromptGenerator::new(args.tokenizer_name())?);

    // Create client
    let max_concurrency = *args.concurrency.iter().max().unwrap_or(&1);
    let client = Arc::new(BenchmarkClient::new(
        args.api_base.clone(),
        args.model.clone(),
        args.api_key.clone(),
        !args.no_ignore_eos,
        args.timeout,
        max_concurrency,
    )?);

    // Set up Ctrl+C handler
    let cancelled = Arc::new(AtomicBool::new(false));
    let cancelled_clone = cancelled.clone();
    ctrlc::set_handler(move || {
        eprintln!("\nReceived Ctrl+C, finishing current requests...");
        cancelled_clone.store(true, Ordering::Relaxed);
    })?;

    let output_dir = args.output_dir();
    let base_path = Path::new(&output_dir);

    // Run benchmarks
    for scenario in &scenarios {
        let scenario_dir = base_path.join(scenario.dir_name());
        std::fs::create_dir_all(&scenario_dir)?;

        eprintln!(
            "\ngenai-bench-rs — {} — model: {}",
            scenario.name(),
            args.model
        );

        let mut all_aggregated: Vec<metrics::AggregatedMetrics> = Vec::new();
        let mut all_errors: Vec<(u32, HashMap<String, usize>)> = Vec::new();
        let mut all_raw_results: Vec<(u32, Vec<metrics::RawRequestResult>)> = Vec::new();
        for &concurrency in &args.concurrency {
            if cancelled.load(Ordering::Relaxed) {
                break;
            }

            let config = RunConfig {
                duration: args.duration,
                max_requests: args.max_requests,
                concurrency,
            };

            let result = run_benchmark(
                client.clone(),
                prompt_generator.clone(),
                scenario.as_ref(),
                &config,
                cancelled.clone(),
            )
            .await;

            all_aggregated.push(result.aggregated.clone());
            all_errors.push((concurrency, result.error_breakdown.clone()));
            all_raw_results.push((concurrency, result.all_requests));

            // Print running summary table
            print_summary_table(&all_aggregated);

            if result.aggregated.error_rate > 0.5 {
                eprintln!(
                    "[WARN] Error rate {:.1}% exceeds 50% at concurrency {}",
                    result.aggregated.error_rate * 100.0,
                    concurrency
                );
            }
        }

        // Write outputs
        write_summary_csv(&scenario_dir.join("summary.csv"), &all_aggregated)?;
        write_detailed_stats_csv(&scenario_dir.join("detailed_stats.csv"), &all_aggregated)?;

        // Write raw_results.json
        let metadata = serde_json::json!({
            "api_base": &args.api_base,
            "model": &args.model,
            "scenario": scenario.name(),
            "duration": format!("{}s", args.duration.as_secs()),
        });
        let raw_json_data: Vec<RawJsonEntry<'_>> = all_raw_results
            .iter()
            .zip(all_aggregated.iter())
            .map(|((conc, reqs), agg)| {
                (
                    *conc,
                    reqs.as_slice(),
                    agg.run_duration_s,
                    agg.output_throughput_server_tps,
                    agg.total_requests,
                    agg.error_count,
                )
            })
            .collect();
        write_raw_json(
            &scenario_dir.join("raw_results.json"),
            &metadata,
            &raw_json_data,
        )?;

        // Print error summary
        print_error_summary(&all_errors);

        eprintln!("\nResults saved to {}", scenario_dir.display());
    }

    Ok(())
}

fn run_plot_command(args: cli::PlotArgs) -> Result<()> {
    let data_path = Path::new(&args.data);

    let plot_types = match args.r#type.as_str() {
        "all" => vec![PlotType::Ttft, PlotType::Ois, PlotType::Latency],
        "ttft" => vec![PlotType::Ttft],
        "ois" => vec![PlotType::Ois],
        "latency" => vec![PlotType::Latency],
        other => {
            return Err(anyhow::anyhow!(
                "Unknown plot type: {}. Use: all, ttft, ois, latency",
                other
            ))
        }
    };

    let config = PlotConfig {
        percentile: args.percentile.clone(),
        plot_types,
    };

    // Read summary.csv files from each scenario subdirectory
    let mut scenario_results: Vec<(String, Vec<metrics::AggregatedMetrics>)> = Vec::new();

    for entry in std::fs::read_dir(data_path)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            let detailed_path = entry.path().join("detailed_stats.csv");
            let summary_path = entry.path().join("summary.csv");
            if summary_path.exists() {
                let name = entry.file_name().to_string_lossy().to_string();
                let results = if detailed_path.exists() {
                    read_detailed_stats_csv(&detailed_path, &summary_path)?
                } else {
                    read_summary_csv(&summary_path)?
                };
                scenario_results.push((name, results));
            }
        }
    }

    let scenario_refs: Vec<(&str, &[metrics::AggregatedMetrics])> = scenario_results
        .iter()
        .map(|(name, results)| (name.as_str(), results.as_slice()))
        .collect();

    generate_plots(data_path, &scenario_refs, &config)?;
    eprintln!("Plots saved to {}", data_path.display());
    Ok(())
}

fn read_summary_csv(path: &Path) -> Result<Vec<metrics::AggregatedMetrics>> {
    let mut rdr = csv::Reader::from_path(path)?;
    let mut results = Vec::new();

    for record in rdr.records() {
        let record = record?;
        let agg = metrics::AggregatedMetrics {
            concurrency: record[0].parse()?,
            run_duration_s: 0.0,
            output_throughput_server_tps: record[8].parse()?,
            rps: record[9].parse()?,
            total_requests: record[10].parse()?,
            error_count: record[11].parse()?,
            error_rate: record[12].parse()?,
            stats: metrics::AggregatedStats {
                ttft: metrics::DistributionStats {
                    mean: record[1].parse()?,
                    p99: record[2].parse()?,
                    ..Default::default()
                },
                tpot: metrics::DistributionStats {
                    mean: record[3].parse()?,
                    p99: record[4].parse()?,
                    ..Default::default()
                },
                e2e_latency: metrics::DistributionStats {
                    mean: record[5].parse()?,
                    p99: record[6].parse()?,
                    ..Default::default()
                },
                input_throughput: metrics::DistributionStats::default(),
                output_throughput: metrics::DistributionStats {
                    mean: record[7].parse()?,
                    ..Default::default()
                },
            },
        };
        results.push(agg);
    }

    Ok(results)
}

fn read_detailed_stats_csv(
    detailed_path: &Path,
    summary_path: &Path,
) -> Result<Vec<metrics::AggregatedMetrics>> {
    // First read summary for server-level metrics
    let summary = read_summary_csv(summary_path)?;

    // Then read detailed stats for full distributions
    let mut rdr = csv::Reader::from_path(detailed_path)?;
    let mut dist_map: HashMap<u32, HashMap<String, metrics::DistributionStats>> = HashMap::new();

    for record in rdr.records() {
        let record = record?;
        let concurrency: u32 = record[0].parse()?;
        let metric_name = record[1].to_string();
        let dist = metrics::DistributionStats {
            min: record[2].parse()?,
            p1: record[3].parse()?,
            p5: record[4].parse()?,
            p10: record[5].parse()?,
            p25: record[6].parse()?,
            p50: record[7].parse()?,
            p75: record[8].parse()?,
            p90: record[9].parse()?,
            p95: record[10].parse()?,
            p99: record[11].parse()?,
            max: record[12].parse()?,
            mean: record[13].parse()?,
            stddev: record[14].parse()?,
        };
        dist_map
            .entry(concurrency)
            .or_default()
            .insert(metric_name, dist);
    }

    // Merge detailed distributions into summary metrics
    let mut results = Vec::new();
    for mut agg in summary {
        if let Some(dists) = dist_map.get(&agg.concurrency) {
            if let Some(d) = dists.get("ttft_s") {
                agg.stats.ttft = d.clone();
            }
            if let Some(d) = dists.get("tpot_s") {
                agg.stats.tpot = d.clone();
            }
            if let Some(d) = dists.get("e2e_latency_s") {
                agg.stats.e2e_latency = d.clone();
            }
            if let Some(d) = dists.get("input_throughput_tps") {
                agg.stats.input_throughput = d.clone();
            }
            if let Some(d) = dists.get("output_throughput_tps") {
                agg.stats.output_throughput = d.clone();
            }
        }
        results.push(agg);
    }

    Ok(results)
}

#[cfg(unix)]
fn check_fd_limit(max_concurrency: u32) {
    use std::process::Command;
    let output = match Command::new("sh").arg("-c").arg("ulimit -n").output() {
        Ok(o) => o,
        Err(_) => return,
    };
    let limit: u32 = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .unwrap_or(256);

    if limit < max_concurrency + 50 {
        eprintln!(
            "[WARN] File descriptor limit ({}) may be too low for concurrency {}. \
             Run `ulimit -n {}` to increase.",
            limit,
            max_concurrency,
            max_concurrency * 2
        );
    }
}
