# syntax=docker/dockerfile:1.7

ARG RGINX_BUILD_VERSION=dev
ARG RGINX_BUILD_REVISION=local
ARG RGINX_BUILD_CREATED=unknown

FROM rust:1.94-trixie AS rust-builder
ARG RGINX_BUILD_VERSION
ARG RGINX_BUILD_REVISION
ARG RGINX_BUILD_CREATED

WORKDIR /workspace

RUN rustup target add wasm32-unknown-unknown \
    && cargo install --locked wasm-bindgen-cli --version 0.2.118

COPY Cargo.toml Cargo.lock ./
COPY crates ./crates

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/workspace/target \
    cargo build --locked --release \
        -p rginx-web \
    && cargo build --locked --release \
        -p rginx-control-console \
        --target wasm32-unknown-unknown \
    && mkdir -p /out \
    && mkdir -p /out/control-console \
    && wasm-bindgen \
        --target web \
        --no-typescript \
        --out-dir /out/control-console \
        --out-name console \
        /workspace/target/wasm32-unknown-unknown/release/rginx_control_console.wasm \
    && cp /workspace/crates/rginx-control-console/static/index.html /out/control-console/index.html \
    && cp /workspace/crates/rginx-control-console/static/console.css /out/control-console/console.css \
    && cp /workspace/target/release/rginx-web /out/rginx-web \
    && strip /out/rginx-web

FROM debian:trixie-slim AS rginx-web
ARG RGINX_BUILD_VERSION
ARG RGINX_BUILD_REVISION
ARG RGINX_BUILD_CREATED

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --system rginx \
    && useradd --system --gid rginx --home /nonexistent --shell /usr/sbin/nologin rginx

LABEL org.opencontainers.image.title="rginx-web" \
      org.opencontainers.image.version="${RGINX_BUILD_VERSION}" \
      org.opencontainers.image.revision="${RGINX_BUILD_REVISION}" \
      org.opencontainers.image.created="${RGINX_BUILD_CREATED}"

COPY --from=rust-builder --chmod=0755 /out/rginx-web /usr/local/bin/rginx-web
COPY --from=rust-builder /out/control-console /opt/rginx/control-console

EXPOSE 8080

ENV RGINX_CONTROL_API_ADDR=0.0.0.0:8080
ENV RGINX_CONTROL_UI_DIR=/opt/rginx/control-console

USER rginx:rginx

ENTRYPOINT ["/usr/local/bin/rginx-web"]
