# Working Preferences and Standards

## Validation gate — definition of "done"

A change is not done until:
1. `cargo fmt --all --check` passes.
2. `cargo clippy --locked --workspace --all-targets -- -D warnings` is clean.
3. `cargo nextest run --locked --workspace --all-features` passes (fallback: `cargo test`).
4. `cargo deny check advisories bans licenses` passes (CI; pre-commit runs advisories and licenses).
5. `cargo build --locked -p contextforge-data-plane --features plugins` succeeds (production feature selection).
6. If the change touches the hot path, update the matching wiki page in `_context/wiki/` in the same change.

CI additionally runs `cargo shear --check-test-targets --deny-warnings --locked`
and `cargo bench --no-run`.

**By change type:**

| Change type | Minimum extra validation |
| --- | --- |
| Docs only | Run `mdbook build _context/wiki` and `mdbook test _context/wiki`; inspect affected headings, tables, and code blocks in the rendered output |
| Routing or request lifecycle | New/updated integration tests in `crates/contextforge-data-plane-lib/tests/` against mock backends |
| Config shape | Schema regeneration (`cargo run -p contextforge-data-plane-apis`) + control-plane compatibility check |
| Plugin behavior | `tests/gateway/plugins.rs` integration coverage plus CPEX/runtime or concrete-plugin tests as appropriate |
| Performance-sensitive paths | Load-test run before and after |

## Code style

- **Idiomatic Rust** — no unnecessary clones, heap allocations, `Arc`, or `Mutex` unless justified by the design.
- Most ContextForge external-dataplane behavior lives in `contextforge-data-plane-lib`. Do not let external-dataplane logic accumulate in the binary crate.
- Typed errors — propagate errors rather than swallowing them silently.
- Keep change size minimal. Every changed line must trace directly to the task at hand.

## Logging (tracing)

- Use `tracing` for all log output.
- **Prefer message-embedded fields**: `level!("method_name - event field = {val} other_field = {other}")`.
  Do **not** use structured field syntax (`, field = val`) for ContextForge external-dataplane logs.
- Keep method/event prefixes stable and reuse the same field names and order for related events.
- `warn!` is for unexpected conditions that need operator attention. Expected user/config misses → `debug!` or `info!`.
- **Never log**: tokens, authorization headers, secrets, Redis key/value bytes, full `UserConfig`, or backend credentials.

## Change discipline

- Make the **minimal change** that solves the problem. No speculative refactors, no added abstractions beyond the task scope.
- Do not clean up surrounding code that is unrelated to the task.
- Do not add error handling for scenarios that cannot happen.
- Always **read relevant code before suggesting or making changes**. Never speculate about code that hasn't been opened.

## Architectural rules (non-negotiable)

- The ContextForge external dataplane is pure routing logic. **No IAM, UI, or metrics-storage concerns.**
- Config access goes through `UserConfigStore` only — never push Redis details into routing code.
- Preserve the published client-facing naming contract. Routing now uses explicit `ServiceRoute` maps; changes must update the publisher, schemas, and tests rather than reintroducing prefix splitting.
- Legacy SSE and older-client initialization/session behavior stay on Python routes. Remaining Rust compatibility code is migration state; do not build new behavior on it.
- Prefer the right architecture over backward compatibility; this project has no external users yet.

## Protocol target

- The external-dataplane contract targets MCP **`2026-07-28`** over **Streamable HTTP**.
- Each request is independent. Do not require a session ID, affinity, or a retained backend transport.
- Tests and examples use `server/discover` and per-request client metadata.
- Do not add compatibility for older protocol versions, legacy `initialize`/session behavior, or legacy SSE. Keep those clients on control-plane/built-in routes.

## AI interaction preferences

- **Read before acting**: always investigate relevant files before making suggestions or edits.
- **Minimal scope**: stay tightly scoped to the task — no unsolicited refactors or cleanups.
- **Plan first for complex tasks**: for changes with multiple moving parts, propose the approach before implementing.
- **Run validation**: run `cargo test` and `cargo clippy` after changes and report results before declaring done.
- **Update the wiki**: when hot-path behavior changes, include the wiki page update in the same task.
- **No hallucination**: if something is unclear, ask rather than guess.


## Branch naming

Format: `user/<github-username>/<kebab-case-summary>` — e.g. `user/alice/fix-session-cleanup`.
Open PRs as draft; mark ready only when implementation, tests, and wiki updates are complete.
