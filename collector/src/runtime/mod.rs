mod benchmark;

use crate::toolchain::LocalToolchain;
use benchlib::comm::messages::{BenchmarkMessage, BenchmarkResult, BenchmarkStats};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use thousands::Separable;

pub use benchmark::BenchmarkFilter;

/// Perform a series of runtime benchmarks using the provided `rustc` compiler.
/// The runtime benchmarks are looked up in `benchmark_dir`, which is expected to be a path
/// to a Cargo crate. All binaries built by that crate will are expected to be runtime benchmark
/// groups that leverage `benchlib`.
pub fn bench_runtime(
    toolchain: LocalToolchain,
    filter: BenchmarkFilter,
    benchmark_dir: PathBuf,
    iterations: u32,
) -> anyhow::Result<()> {
    let suite = benchmark::discover_benchmarks(&toolchain, &benchmark_dir)?;

    let total_benchmark_count = suite.total_benchmark_count();
    let filtered = suite.filtered_benchmark_count(&filter);
    println!(
        "Executing {} benchmarks ({} filtered out)\n",
        filtered,
        total_benchmark_count - filtered
    );

    let mut benchmark_index = 0;
    for binary in suite.groups {
        for message in execute_runtime_benchmark_binary(&binary.binary, &filter, iterations)? {
            let message = message.map_err(|err| {
                anyhow::anyhow!(
                    "Cannot parse BenchmarkMessage from benchmark {}: {err:?}",
                    binary.binary.display()
                )
            })?;
            match message {
                BenchmarkMessage::Result(result) => {
                    benchmark_index += 1;
                    println!(
                        "Finished {}/{} ({}/{})",
                        binary.name(),
                        result.name,
                        benchmark_index,
                        filtered
                    );
                    print_stats(&result);
                }
            }
        }
    }

    Ok(())
}

/// Starts executing a single runtime benchmark group defined in a binary crate located in
/// `runtime-benchmarks`. The binary is expected to use benchlib's `BenchmarkGroup` to execute
/// a set of runtime benchmarks and print `BenchmarkMessage`s encoded as JSON, one per line.
fn execute_runtime_benchmark_binary(
    binary: &Path,
    filter: &BenchmarkFilter,
    iterations: u32,
) -> anyhow::Result<impl Iterator<Item = anyhow::Result<BenchmarkMessage>>> {
    // Turn off ASLR
    let mut command = Command::new("setarch");
    command.arg(std::env::consts::ARCH);
    command.arg("-R");
    command.arg(binary);
    command.arg("run");
    command.arg("--iterations");
    command.arg(&iterations.to_string());

    if let Some(ref exclude) = filter.exclude {
        command.args(&["--exclude", exclude]);
    }
    if let Some(ref include) = filter.include {
        command.args(&["--include", include]);
    }

    command.stdout(Stdio::piped());
    let mut child = command.spawn()?;
    let stdout = child.stdout.take().unwrap();

    let reader = BufReader::new(stdout);
    let iterator = reader.lines().map(|line| {
        line.and_then(|line| Ok(serde_json::from_str::<BenchmarkMessage>(&line)?))
            .map_err(|err| err.into())
    });
    Ok(iterator)
}

fn calculate_mean<I: Iterator<Item = f64> + Clone>(iter: I) -> f64 {
    let sum: f64 = iter.clone().sum();
    let count = iter.count();
    sum / count as f64
}

fn print_stats(result: &BenchmarkResult) {
    fn print_metric<F: Fn(&BenchmarkStats) -> Option<u64>>(
        result: &BenchmarkResult,
        name: &str,
        f: F,
    ) {
        let name = format!("[{name}]");
        let has_data = result.stats.iter().map(&f).all(|v| v.is_some());
        if has_data {
            let f = |stats: &BenchmarkStats| -> u64 { f(stats).unwrap() };
            let mean = calculate_mean(result.stats.iter().map(&f).map(|v| v as f64));
            let stddev = calculate_mean(
                result
                    .stats
                    .iter()
                    .map(&f)
                    .map(|v| (v as f64 - mean).powf(2.0)),
            )
            .sqrt();

            println!(
                "{name:>18}: {:>16} (+/- {:>11})",
                (mean as u64).separate_with_commas(),
                (stddev as u64).separate_with_commas()
            );
        } else {
            println!("{name:>20}: Not available");
        }
    }

    print_metric(result, "Instructions", |m| m.instructions);
    print_metric(result, "Cycles", |m| m.cycles);
    print_metric(result, "Wall time [µs]", |m| {
        Some(m.wall_time.as_micros() as u64)
    });
    print_metric(result, "Branch misses", |m| m.branch_misses);
    print_metric(result, "Cache misses", |m| m.cache_misses);
    println!();
}
