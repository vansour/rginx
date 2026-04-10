FROM debian:trixie-slim

ENV DEBIAN_FRONTEND=noninteractive
ENV CARGO_HOME=/usr/local/cargo
ENV RUSTUP_HOME=/usr/local/rustup
ENV PATH=/usr/local/cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin

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

RUN curl -fsSL https://sh.rustup.rs | sh -s -- -y --default-toolchain 1.85.0 --profile minimal \
    && rustc --version \
    && cargo --version

WORKDIR /work
COPY . /work

ENTRYPOINT ["python3", "/work/scripts/nginx_compare.py", "--workspace", "/work", "--out-dir", "/out"]
