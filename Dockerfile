# syntax=docker/dockerfile:1
# aka — 感知所有代码的知识引擎。
#
# 多阶段：
#   engine-builder — 构建 AKA engine embedded 静态库
#   rust-builder   — cargo release 构建内置 embedded-engine 的内部 runtime
#   rust-cross     — （可选，--target rust-cross 单独构建）交叉编译 x86_64 linux runtime
#   runtime        — git + aka；非 root，数据卷 /data
#
# 构建 / 运行：
#   docker build -t aka:0.1.0 .
#   docker run -d -p 127.0.0.1:4111:4111 -v aka-data:/data aka:0.1.0
# 详见 docs/deploy.md。

# ---------- Stage 1: native AKA engine ----------
FROM debian:bookworm AS engine-builder
ARG AKA_ENGINE_REPO=https://github.com/caork/aka-engine.git
ARG AKA_ENGINE_REF=7c5e380bcc8ecc8b7a457b122763774e3164301c
RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates git build-essential pkg-config zlib1g-dev && \
    rm -rf /var/lib/apt/lists/*
WORKDIR /src
RUN git clone "${AKA_ENGINE_REPO}" aka-engine && \
    cd aka-engine && \
    git checkout "${AKA_ENGINE_REF}" && \
    make -f Makefile.cbm libaka-engine

# ---------- Stage 2: Rust builder ----------
FROM rust:1.93-bookworm AS rust-builder
ENV CARGO_NET_RETRY=10 \
    CARGO_TERM_COLOR=never \
    CARGO_BUILD_JOBS=1
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY apps/cli ./apps/cli
COPY --from=engine-builder /src/aka-engine engine/aka-engine-src
RUN cargo build --release -p aka-cli --features embedded-engine && \
    strip target/release/aka

# ---------- Stage 2b: x86_64 交叉编译（不在默认链路上；docker build --target rust-cross 时才执行） ----------
FROM rust-builder AS rust-cross
RUN apt-get update && \
    apt-get install -y --no-install-recommends gcc-x86-64-linux-gnu g++-x86-64-linux-gnu && \
    rm -rf /var/lib/apt/lists/* && \
    rustup target add x86_64-unknown-linux-gnu
ENV CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=x86_64-linux-gnu-gcc \
    CC_x86_64_unknown_linux_gnu=x86_64-linux-gnu-gcc \
    CXX_x86_64_unknown_linux_gnu=x86_64-linux-gnu-g++ \
    AR_x86_64_unknown_linux_gnu=x86_64-linux-gnu-ar
RUN cargo build --release -p aka-cli --features embedded-engine --target x86_64-unknown-linux-gnu && \
    x86_64-linux-gnu-strip target/x86_64-unknown-linux-gnu/release/aka

# ---------- Stage 3: runtime ----------
FROM debian:bookworm-slim AS runtime
LABEL org.opencontainers.image.title="aka" \
      org.opencontainers.image.description="Code-omniscient knowledge engine (AKA engine parse -> tantivy BM25 + SQLite/CSR graph; internal runtime/MCP/HTTP)." \
      org.opencontainers.image.source="https://github.com/caork/aka" \
      org.opencontainers.image.licenses="MIT" \
      org.opencontainers.image.version="0.1.0"

# git：HTTP import {kind:"git"} 在容器内 clone 时需要
RUN apt-get update && \
    apt-get install -y --no-install-recommends git ca-certificates && \
    rm -rf /var/lib/apt/lists/*

COPY --from=rust-builder /src/target/release/aka /usr/local/bin/aka
# 冒烟样本：docker exec <ctr> aka analyze /opt/aka/fixtures-demo
COPY fixtures/demo-ts /opt/aka/fixtures-demo

RUN useradd --create-home --uid 10001 aka && \
    mkdir -p /data && \
    chown -R aka:aka /data /opt/aka

ENV AKA_HOME=/data

USER aka
VOLUME /data
EXPOSE 4111
CMD ["aka", "serve", "--addr", "0.0.0.0:4111"]
