use std::hint::black_box;

use contextforge_data_plane_benchmarks::{BenchmarkRun, Scenario, execute, setup};
use gungraun::{Callgrind, EventKind, prelude::*};

fn setup_discover() -> BenchmarkRun {
    setup(Scenario::Discover)
}

fn setup_excessive_standard_headers() -> BenchmarkRun {
    setup(Scenario::ExcessiveStandardHeaders)
}

fn setup_unknown_tool() -> BenchmarkRun {
    setup(Scenario::UnknownTool)
}

fn setup_parameter_header_mismatch() -> BenchmarkRun {
    setup(Scenario::ParameterHeaderMismatch)
}

fn teardown(benchmark: BenchmarkRun) {
    drop(benchmark);
}

#[library_benchmark(setup = setup_discover, teardown = teardown)]
fn discover(benchmark: BenchmarkRun) -> BenchmarkRun {
    black_box(execute(black_box(benchmark)))
}

#[library_benchmark(setup = setup_excessive_standard_headers, teardown = teardown)]
fn excessive_standard_headers(benchmark: BenchmarkRun) -> BenchmarkRun {
    black_box(execute(black_box(benchmark)))
}

#[library_benchmark(setup = setup_unknown_tool, teardown = teardown)]
fn unknown_tool(benchmark: BenchmarkRun) -> BenchmarkRun {
    black_box(execute(black_box(benchmark)))
}

#[library_benchmark(setup = setup_parameter_header_mismatch, teardown = teardown)]
fn parameter_header_mismatch(benchmark: BenchmarkRun) -> BenchmarkRun {
    black_box(execute(black_box(benchmark)))
}

library_benchmark_group!(
    name = request_path;
    benchmarks = discover, excessive_standard_headers, unknown_tool, parameter_header_mismatch
);

main!(
    config = LibraryBenchmarkConfig::default().tool(
        Callgrind::with_args(["--cache-sim=no"]).soft_limits([(EventKind::Ir, 5.0)])
    );
    library_benchmark_groups = request_path
);
