FROM rust:1.96.1 AS builder
WORKDIR /app

RUN <<EOF
apt update
apt install -y ca-certificates protobuf-compiler
EOF

COPY . .
# Default: production build with plugin factories, no testing-only helpers.
# Override with --build-arg FEATURES=plugins,with_tools for dev/compose images.
ARG FEATURES=plugins
RUN --mount=type=cache,id=cargo,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=cargo-git,target=/usr/local/cargo/git,sharing=locked \
    cargo fetch --locked
RUN --mount=type=cache,id=cargo,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=cargo-git,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,id=contextforge-data-plane-target,target=/app/target,sharing=locked \
    cargo build --locked --release -p contextforge-data-plane --features "${FEATURES}" \
    && mkdir -p /out \
    && cp /app/target/release/contextforge-data-plane /out/contextforge-data-plane

FROM debian:trixie-slim AS runtime
RUN <<EOF
apt update
apt upgrade -y
apt install -y python3
EOF

WORKDIR /
LABEL org.opencontainers.image.source=https://github.com/contextforge-org/contextforge-data-plane
LABEL org.opencontainers.image.description="contextforge-data-plane - open source experimental data plane for ContextForge."
ENTRYPOINT ["/contextforge-data-plane"]

FROM runtime AS conformance-prebuilt
COPY --from=prebuilt /contextforge-data-plane /contextforge-data-plane

FROM runtime
COPY --from=builder /out/contextforge-data-plane /contextforge-data-plane
