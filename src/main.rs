mod cli;

use clap::Parser;
use cli::{Cli, Commands};

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Benchmark(args) => {
            println!("Benchmark mode: model={}, scenarios={:?}, concurrency={:?}",
                args.model, args.scenario, args.concurrency);
            println!("Output dir: {}", args.output_dir());
        }
        Commands::Plot(args) => {
            println!("Plot mode: data={}, percentile={}, type={}",
                args.data, args.percentile, args.r#type);
        }
    }
}
