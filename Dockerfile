# syntax=docker/dockerfile:1
# aka — 感知所有代码的知识引擎（非商用：上游 GitNexus 为 PolyForm Noncommercial 1.0）。
#
# 多阶段：
#   rust-builder   — cargo release 构建 aka 二进制（native arch）
#   rust-cross     — （可选，--target rust-cross 单独构建）交叉编译 x86_64 linux 二进制
#   engine-builder — node 22 里装 engine 依赖并编译 tree-sitter 原生模块
#   runtime        — node22 slim + git + aka + engine；非 root，数据卷 /data
#
# 构建 / 运行：
#   docker build -t aka:0.1.0 .
#   docker run -d -p 127.0.0.1:4111:4111 -v aka-data:/data aka:0.1.0
# 详见 docs/deploy.md。

# ---------- Stage 1: Rust builder ----------
FROM rust:1.93-bookworm AS rust-builder
ENV CARGO_NET_RETRY=10 \
    CARGO_TERM_COLOR=never
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY apps/cli ./apps/cli
RUN cargo build --release -p aka-cli && \
    strip target/release/aka

# ---------- Stage 1b: x86_64 交叉编译（不在默认链路上；docker build --target rust-cross 时才执行） ----------
FROM rust-builder AS rust-cross
RUN apt-get update && \
    apt-get install -y --no-install-recommends gcc-x86-64-linux-gnu g++-x86-64-linux-gnu && \
    rm -rf /var/lib/apt/lists/* && \
    rustup target add x86_64-unknown-linux-gnu
ENV CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=x86_64-linux-gnu-gcc \
    CC_x86_64_unknown_linux_gnu=x86_64-linux-gnu-gcc \
    CXX_x86_64_unknown_linux_gnu=x86_64-linux-gnu-g++ \
    AR_x86_64_unknown_linux_gnu=x86_64-linux-gnu-ar
RUN cargo build --release -p aka-cli --target x86_64-unknown-linux-gnu && \
    x86_64-linux-gnu-strip target/x86_64-unknown-linux-gnu/release/aka

# ---------- Stage 2: engine 依赖（TS sidecar；tree-sitter 原生模块在此编译） ----------
# 注意用完整版 node 镜像：postinstall 编译 tree-sitter grammar 需要 python3/make/g++。
FROM node:22-bookworm AS engine-builder
ENV SCARF_ANALYTICS=false \
    npm_config_fund=false \
    npm_config_audit=false \
    npm_config_update_notifier=false
WORKDIR /opt/aka/engine
# 先 shared（gitnexus 以 file:../gitnexus-shared 依赖它的 dist）
COPY engine/gitnexus-shared ./gitnexus-shared
RUN cd gitnexus-shared && npm install && npm run build
COPY engine/gitnexus ./gitnexus
RUN cd gitnexus && npm install

# ---------- Stage 3: runtime ----------
FROM node:22-bookworm-slim AS runtime
LABEL org.opencontainers.image.title="aka" \
      org.opencontainers.image.description="Code-omniscient knowledge engine (tree-sitter parse -> tantivy BM25 + SQLite/CSR graph; CLI/MCP/HTTP). Noncommercial use only." \
      org.opencontainers.image.source="https://github.com/caork/aka" \
      org.opencontainers.image.licenses="PolyForm-Noncommercial-1.0.0" \
      org.opencontainers.image.version="0.1.0"

# git：HTTP import {kind:"git"} 在容器内 clone 时需要
RUN apt-get update && \
    apt-get install -y --no-install-recommends git ca-certificates && \
    rm -rf /var/lib/apt/lists/*

COPY --from=rust-builder /src/target/release/aka /usr/local/bin/aka
COPY --from=engine-builder /opt/aka/engine /opt/aka/engine
# 冒烟样本：docker exec <ctr> aka analyze /opt/aka/fixtures-demo
COPY fixtures/demo-ts /opt/aka/fixtures-demo

RUN useradd --create-home --uid 10001 aka && \
    mkdir -p /data && \
    chown -R aka:aka /data /opt/aka

ENV AKA_HOME=/data \
    AKA_ENGINE_DIR=/opt/aka/engine \
    SCARF_ANALYTICS=false \
    npm_config_update_notifier=false

USER aka
VOLUME /data
EXPOSE 4111
CMD ["aka", "serve", "--addr", "0.0.0.0:4111"]
