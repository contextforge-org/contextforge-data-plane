# Performance and Load Testing

## CI Instruction-Count Benchmarks

The `contextforge-data-plane-benchmarks` workspace crate contains a small
[Gungraun](https://gungraun.github.io/gungraun/latest/html/index.html) suite for
deterministic regression checks. It measures complete in-process request and
response handling through `Gateway::into_router` with fixed authorization and
user configuration. It does not start Redis, backend servers, Docker, or the
`cf-integration` harness.

The six cases cover modern `server/discover`, early MCP standard-header limit
rejection, an unknown tool route, a routed tool rejected by parameter-header
validation, a valid routed tool call, and a published resource read. The final
two use an unsupported backend URL scheme so they exercise header construction
and upstream client setup, then fail deterministically before DNS or socket I/O.
Their fixed catalog contains one virtual host, four backends, 256 tool routes,
64 resource routes, 32 prompt routes, and 32 resource-template routes. Setup and
teardown are excluded from measurement.

CI runs each benchmark once under Callgrind with cache simulation disabled. A
successful `main` run uploads an immutable baseline named with its commit SHA.
A pull request downloads the baseline for its exact base SHA and normally runs
only the candidate. If the artifact is missing, expired, or was produced by a
different compiler, runner, Valgrind, architecture, or C library environment,
CI safely recomputes the exact base on the current runner before comparison.
The job fails when executed instructions (`Ir`) increase by more than 5%.
Gungraun instruction counts are for relative regression detection; they are not
wall-clock latency or throughput claims. The initial harness change runs without
comparison because its base has no benchmark target.

Gungraun requires Linux, Valgrind, debug symbols, and a `gungraun-runner` version
matching the `gungraun` dependency. CI installs these automatically. On Linux,
install Valgrind and `gungraun-runner` 0.19.4, then run:

```bash
make benchmark
```

The default local baseline is named `local`; override `GUNGRAUN_ARGS` to select
another baseline or filter. macOS cannot run Valgrind natively, so use the CI
job there. Keep new CI cases deterministic, service-free, and small. Use the
load harness below for full backend round trips and before/after throughput or
latency measurements.

Load testing is owned by [`cf-integration`](https://crates.io/crates/cf-integration).
The commands below match **0.5.0**, the version pinned by this repository's
conformance workflow. The old `scripts/cf-integration.sh` wrapper is no longer
in this repository.

## Setup and Smoke Test

Use Docker with enough resources for the selected topology. Install the pinned
CLI and check its command reference:

```bash
cargo binstall cf-integration@0.5.0 --no-confirm
cf-integration load run --help
```

The standalone lane starts the external dataplane, Redis, nginx, and a fixture
without the control plane. It supplies a known published catalog so load setup
does not depend on the unsupported external `tools/list` method:

```bash
CF_INTEGRATION_DIR="$PWD/.integration" \
CF_DATAPLANE_REPO="$PWD" CF_DATAPLANE_REF="$(git rev-parse HEAD)" \
  cf-integration load run --lane external --client-era modern --standalone \
  --users 1 --spawn-rate 1 --run-time 10s
```

The harness checks out the selected committed revision; commit changes before
comparing them. All external examples target modern MCP `2026-07-28`.

## Load Settings

```bash
CF_INTEGRATION_DIR="$PWD/.integration" \
CF_DATAPLANE_REPO="$PWD" CF_DATAPLANE_REF="$(git rev-parse HEAD)" \
  cf-integration load run --lane external --client-era modern --standalone \
  --users 20 --spawn-rate 5 --run-time 2m
```

| Setting | CLI flag | Default |
| --- | --- | --- |
| Concurrent users | `--users` | `100` |
| Spawn rate | `--spawn-rate` | `10` users/second |
| Duration | `--run-time` | `5m` |
| Short workload preset | `--smoke` | Off; explicit load settings take precedence. |
| Diagnostic observability stack | `--observability` | Off, to avoid changing benchmark overhead. |

`LOCUST_USERS`, `LOCUST_SPAWN_RATE`, and `LOCUST_RUN_TIME` also configure the
workload; CLI values take precedence. Durations accept ordered `h`, `m`, and `s`
components, such as `2m30s`. The pinned CLI runs Locust headless; the old
`LOCUST_MODE=web` recipe does not switch this command into a web UI.

HTML, CSV, and `locust.log` are written beneath
`CF_INTEGRATION_DIR/reports/load/`. Record the exact dataplane and harness
versions alongside the report.

## Full-Stack and Built-In Comparisons

Omit `--standalone` to include the control plane and its publication path:

```bash
cf-integration load run --lane external --client-era modern \
  --users 20 --spawn-rate 5 --run-time 2m
cf-integration load run --lane builtin --client-era modern \
  --users 20 --spawn-rate 5 --run-time 2m
```

Use equivalent backends, tools, hardware, authentication, policy, cache settings,
and client metadata for comparisons. A full-stack failure while discovering or
publishing the catalog is a setup failure, not a throughput measurement.
Standalone measurements omit control-plane publication and must be labeled as
such. See the [pinned harness documentation](https://github.com/contextforge-org/contextforge-dev-tools/blob/v0.5.0/README.md)
for topology and source selection.

## Benchmark Controls

Report the publisher interval and
`CF_DATAPLANE_USER_CONFIG_CACHE_EXPIRY_SECONDS` explicitly. Cache expiry `0`
forces a Redis read on every request and is useful for functional checks; the
Rust default is `60`. Keep caching, publication, plugins, and telemetry identical
between before/after measurements. A publisher interval is irrelevant to a
standalone snapshot that is not being republished during the run.

Record request rate, latency percentiles, failures, and resource usage after
warmup. Do not infer correctness or protocol coverage from successful load;
run [workspace and conformance checks](testing.md) separately.
