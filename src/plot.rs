use anyhow::Result;
use plotters::prelude::*;
use std::path::Path;

use crate::metrics::AggregatedMetrics;

pub enum PlotType {
    Ttft,
    Ois,
    Latency,
}

pub struct PlotConfig {
    pub percentile: String,
    pub plot_types: Vec<PlotType>,
}

pub fn generate_plots(
    output_dir: &Path,
    scenario_results: &[(&str, &[AggregatedMetrics])],
    config: &PlotConfig,
) -> Result<()> {
    for plot_type in &config.plot_types {
        match plot_type {
            PlotType::Ttft => plot_ttft_vs_throughput(output_dir, scenario_results, &config.percentile)?,
            PlotType::Ois => plot_ois_vs_throughput(output_dir, scenario_results, &config.percentile)?,
            PlotType::Latency => plot_latency_vs_rps(output_dir, scenario_results, &config.percentile)?,
        }
    }
    Ok(())
}

fn get_percentile_value(
    agg: &AggregatedMetrics,
    field: &str,
    percentile: &str,
    higher_is_better: bool,
) -> f64 {
    let stats = match field {
        "ttft" => &agg.stats.ttft,
        "tpot" => &agg.stats.tpot,
        "e2e_latency" => &agg.stats.e2e_latency,
        "output_throughput" => &agg.stats.output_throughput,
        "input_throughput" => &agg.stats.input_throughput,
        _ => return 0.0,
    };

    // Mirror percentile for higher-is-better metrics (worst case)
    let effective_percentile = if higher_is_better {
        match percentile {
            "p99" => "p1",
            "p90" => "p10",
            _ => percentile,
        }
    } else {
        percentile
    };

    match effective_percentile {
        "mean" => stats.mean,
        "p1" => stats.p1,
        "p5" => stats.p5,
        "p10" => stats.p10,
        "p25" => stats.p25,
        "p50" => stats.p50,
        "p75" => stats.p75,
        "p90" => stats.p90,
        "p95" => stats.p95,
        "p99" => stats.p99,
        _ => stats.mean,
    }
}

fn plot_line_chart(
    path: &Path,
    title: &str,
    x_label: &str,
    y_label: &str,
    series: &[(&str, Vec<(f64, f64)>)],
) -> Result<()> {
    let root = SVGBackend::new(path, (800, 500)).into_drawing_area();
    root.fill(&WHITE)?;

    let all_x: Vec<f64> = series.iter().flat_map(|(_, pts)| pts.iter().map(|(x, _)| *x)).collect();
    let all_y: Vec<f64> = series.iter().flat_map(|(_, pts)| pts.iter().map(|(_, y)| *y)).collect();

    if all_x.is_empty() || all_y.is_empty() {
        return Ok(());
    }

    let x_min = all_x.iter().cloned().fold(f64::INFINITY, f64::min);
    let x_max = all_x.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let y_min = all_y.iter().cloned().fold(f64::INFINITY, f64::min);
    let y_max = all_y.iter().cloned().fold(f64::NEG_INFINITY, f64::max);

    let x_margin = (x_max - x_min) * 0.1;
    let y_margin = (y_max - y_min) * 0.1;

    let mut chart = ChartBuilder::on(&root)
        .caption(title, ("sans-serif", 20))
        .margin(10)
        .x_label_area_size(40)
        .y_label_area_size(60)
        .build_cartesian_2d(
            (x_min - x_margin)..(x_max + x_margin),
            (y_min - y_margin)..(y_max + y_margin),
        )?;

    chart.configure_mesh()
        .x_desc(x_label)
        .y_desc(y_label)
        .draw()?;

    let colors = [&BLUE, &RED, &GREEN, &MAGENTA, &CYAN];

    for (i, (name, points)) in series.iter().enumerate() {
        let color = colors[i % colors.len()];
        chart.draw_series(LineSeries::new(
            points.iter().cloned(),
            color.stroke_width(2),
        ))?
        .label(*name)
        .legend(move |(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], color.stroke_width(2)));
    }

    if series.len() > 1 {
        chart.configure_series_labels()
            .border_style(BLACK)
            .draw()?;
    }

    root.present()?;
    Ok(())
}

fn plot_ttft_vs_throughput(
    output_dir: &Path,
    scenario_results: &[(&str, &[AggregatedMetrics])],
    percentile: &str,
) -> Result<()> {
    let mut series = Vec::new();
    for (name, results) in scenario_results {
        let points: Vec<(f64, f64)> = results
            .iter()
            .map(|agg| {
                let x = agg.output_throughput_server_tps;
                let y = get_percentile_value(agg, "ttft", percentile, false);
                (x, y)
            })
            .collect();
        series.push((*name, points));
    }

    let title = format!("TTFT ({}) vs Output Throughput of Server", percentile);
    plot_line_chart(
        &output_dir.join("ttft_vs_throughput.svg"),
        &title,
        "Output Throughput of Server (tokens/s)",
        &format!("TTFT ({}) (s)", percentile),
        &series.iter().map(|(n, p)| (*n, p.clone())).collect::<Vec<_>>(),
    )
}

fn plot_ois_vs_throughput(
    output_dir: &Path,
    scenario_results: &[(&str, &[AggregatedMetrics])],
    percentile: &str,
) -> Result<()> {
    let mirrored = match percentile {
        "p99" => "p1",
        "p90" => "p10",
        _ => percentile,
    };

    let mut series = Vec::new();
    for (name, results) in scenario_results {
        let points: Vec<(f64, f64)> = results
            .iter()
            .map(|agg| {
                let x = agg.output_throughput_server_tps;
                let y = get_percentile_value(agg, "output_throughput", percentile, true);
                (x, y)
            })
            .collect();
        series.push((*name, points));
    }

    let title = format!(
        "Output Throughput per Request ({} worst-case) vs Output Throughput of Server",
        mirrored
    );
    plot_line_chart(
        &output_dir.join("ois_vs_throughput.svg"),
        &title,
        "Output Throughput of Server (tokens/s)",
        &format!("Output Throughput per Request ({} worst-case) (tokens/s)", mirrored),
        &series.iter().map(|(n, p)| (*n, p.clone())).collect::<Vec<_>>(),
    )
}

fn plot_latency_vs_rps(
    output_dir: &Path,
    scenario_results: &[(&str, &[AggregatedMetrics])],
    percentile: &str,
) -> Result<()> {
    let mut series = Vec::new();
    for (name, results) in scenario_results {
        let points: Vec<(f64, f64)> = results
            .iter()
            .map(|agg| {
                let x = agg.rps;
                let y = get_percentile_value(agg, "e2e_latency", percentile, false);
                (x, y)
            })
            .collect();
        series.push((*name, points));
    }

    let title = format!("E2E Latency ({}) vs RPS", percentile);
    plot_line_chart(
        &output_dir.join("e2e_latency_vs_rps.svg"),
        &title,
        "Request Throughput (RPS)",
        &format!("E2E Latency ({}) (s)", percentile),
        &series.iter().map(|(n, p)| (*n, p.clone())).collect::<Vec<_>>(),
    )
}
