# syntax=docker/dockerfile:1.7

ARG RGINX_BUILD_VERSION=dev
ARG RGINX_BUILD_REVISION=local
ARG RGINX_BUILD_CREATED=unknown

FROM rust:1.94-trixie AS rust-builder
ARG RGINX_BUILD_VERSION
ARG RGINX_BUILD_REVISION
ARG RGINX_BUILD_CREATED

WORKDIR /workspace

COPY Cargo.toml Cargo.lock ./
COPY crates ./crates

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/workspace/target \
    cargo build --locked --release \
        -p rginx-control-api \
        -p rginx-control-worker \
    && mkdir -p /out \
    && cp /workspace/target/release/rginx-control-api /out/rginx-control-api \
    && cp /workspace/target/release/rginx-control-worker /out/rginx-control-worker \
    && strip /out/rginx-control-api /out/rginx-control-worker

FROM node:24-trixie AS console-builder
WORKDIR /app

COPY web/console/package*.json ./
RUN --mount=type=cache,target=/root/.npm npm ci

COPY web/console ./
RUN npm run build

FROM debian:trixie-slim AS rginx-control
ARG RGINX_BUILD_VERSION
ARG RGINX_BUILD_REVISION
ARG RGINX_BUILD_CREATED

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --system rginx \
    && useradd --system --gid rginx --home /nonexistent --shell /usr/sbin/nologin rginx

LABEL org.opencontainers.image.title="rginx-control" \
      org.opencontainers.image.version="${RGINX_BUILD_VERSION}" \
      org.opencontainers.image.revision="${RGINX_BUILD_REVISION}" \
      org.opencontainers.image.created="${RGINX_BUILD_CREATED}"

COPY --from=rust-builder --chmod=0755 /out/rginx-control-api /usr/local/bin/rginx-control-api
COPY --from=rust-builder --chmod=0755 /out/rginx-control-worker /usr/local/bin/rginx-control-worker
COPY --from=console-builder /app/dist /opt/rginx/control-console
COPY --chmod=0755 docker/control-plane/rginx-control-entrypoint.sh /usr/local/bin/rginx-control

EXPOSE 8080

ENV RGINX_CONTROL_API_ADDR=0.0.0.0:8080
ENV RGINX_CONTROL_UI_DIR=/opt/rginx/control-console

USER rginx:rginx

ENTRYPOINT ["/usr/local/bin/rginx-control"]
CMD ["api"]

FROM debian:trixie-slim AS nginx-compare

ENV DEBIAN_FRONTEND=noninteractive
ENV CARGO_HOME=/usr/local/cargo
ENV RUSTUP_HOME=/usr/local/rustup
ENV PATH=/usr/local/cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
ARG RUST_TOOLCHAIN=1.94.1
ARG RGINX_BUILD_VERSION
ARG RGINX_BUILD_REVISION
ARG RGINX_BUILD_CREATED

LABEL org.opencontainers.image.title="rginx-nginx-compare" \
      org.opencontainers.image.version="${RGINX_BUILD_VERSION}" \
      org.opencontainers.image.revision="${RGINX_BUILD_REVISION}" \
      org.opencontainers.image.created="${RGINX_BUILD_CREATED}"

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        apache2-utils \
        build-essential \
        ca-certificates \
        cmake \
        curl \
        git \
        libpcre2-dev \
        libssl-dev \
        ninja-build \
        openssl \
        perl \
        pkg-config \
        python3-pip \
        python3 \
        xz-utils \
    && rm -rf /var/lib/apt/lists/*

RUN pip3 install --no-cache-dir --break-system-packages grpcio==1.71.0

RUN curl -fsSL https://sh.rustup.rs | sh -s -- -y --default-toolchain "${RUST_TOOLCHAIN}" --profile minimal \
    && rustc --version \
    && cargo --version

WORKDIR /work
COPY . /work

ENTRYPOINT ["python3", "/work/scripts/nginx_compare.py", "--workspace", "/work", "--out-dir", "/out"]
