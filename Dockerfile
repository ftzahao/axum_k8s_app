# syntax=docker/dockerfile:1

# ============ Builder 阶段 ============
FROM rust:1-bookworm AS builder
WORKDIR /app

# 1. 只拷贝 manifest，先用占位源码构建一次：
#    Cargo 依赖层被缓存，后续改业务代码不会重编几百个依赖
COPY Cargo.toml Cargo.lock Toasty.toml ./
RUN mkdir -p src/bin && \
    echo 'fn main() {}' > src/main.rs && \
    echo 'fn main() {}' > src/bin/cli.rs && \
    cargo build --release --locked && \
    rm -rf src

# 2. 拷入真实源码，最终构建
COPY src ./src
RUN cargo build --release --locked

# ============ Runtime 阶段 ============
FROM debian:bookworm-slim AS runtime

# 非 root 用户
RUN groupadd --system app && useradd --system --gid app app

WORKDIR /app

# 服务二进制（server + 迁移 CLI）
COPY --from=builder /app/target/release/axum_k8s_app /usr/local/bin/app
COPY --from=builder /app/target/release/cli /usr/local/bin/cli

# 迁移 CLI 运行时需要：Toasty.toml（Config::load 定位迁移目录）+ 迁移 SQL 文件
COPY Toasty.toml ./
COPY toasty/ ./toasty/

USER app
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/app"]
