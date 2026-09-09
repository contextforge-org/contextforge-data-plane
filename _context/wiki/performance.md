# Performance and Load Testing

Load testing is owned by [`cf-integration`](https://crates.io/crates/cf-integration).
The commands below match **0.3.1**, the version pinned by this repository's
conformance workflow. The old `scripts/cf-integration.sh` wrapper is no longer
in this repository.

## Setup and Smoke Test

Use Docker with enough resources for the selected topology. Install the pinned
CLI and check its command reference:

```bash
cargo binstall cf-integration@0.3.1 --no-confirm
cf-integration load --help
```

The standalone lane starts the external dataplane, Redis, nginx, and a fixture
without the control plane. It supplies a known published catalog so load setup
does not depend on the unsupported external `tools/list` method:

```bash
CF_INTEGRATION_DIR="$PWD/.integration" \
CF_DATAPLANE_REPO="$PWD" CF_DATAPLANE_REF="$(git rev-parse HEAD)" \
  cf-integration load --lane external --protocol-version modern --standalone \
  --users 1 --spawn-rate 1 --run-time 10s
```

The harness checks out the selected committed revision; commit changes before
comparing them. All external examples target modern MCP `2026-07-28`.

## Load Settings

```bash
CF_INTEGRATION_DIR="$PWD/.integration" \
CF_DATAPLANE_REPO="$PWD" CF_DATAPLANE_REF="$(git rev-parse HEAD)" \
  cf-integration load --lane external --protocol-version modern --standalone \
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
cf-integration load --lane external --protocol-version modern \
  --users 20 --spawn-rate 5 --run-time 2m
cf-integration load --lane builtin --protocol-version modern \
  --users 20 --spawn-rate 5 --run-time 2m
```

Use equivalent backends, tools, hardware, authentication, policy, cache settings,
and client metadata for comparisons. A full-stack failure while discovering or
publishing the catalog is a setup failure, not a throughput measurement.
Standalone measurements omit control-plane publication and must be labeled as
such. See the [pinned harness documentation](https://github.com/contextforge-org/contextforge-dev-tools/blob/v0.3.1/README.md)
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
