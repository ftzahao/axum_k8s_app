# syntax=docker/dockerfile:1

# ============ Builder 阶段 ============
FROM rust:1.97-alpine AS builder

# ring（postgres TLS 链路）是 C 代码，官方 rust:alpine 镜像不预装编译器（Debian 版才预装）
RUN apk add --no-cache build-base

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
FROM alpine:3.24 AS runtime

# 非 root 用户（数字 UID：kubelet runAsNonRoot 校验要求可验证的数值 UID；busybox 语法）
RUN addgroup -S -g 1001 app && adduser -S -u 1001 -G app app

WORKDIR /app

# 服务二进制（server + 迁移 CLI）
COPY --from=builder /app/target/release/axum_k8s_app /usr/local/bin/app
COPY --from=builder /app/target/release/cli /usr/local/bin/cli

# 迁移 CLI 运行时需要：Toasty.toml（Config::load 定位迁移目录）+ 迁移 SQL 文件
COPY Toasty.toml ./
COPY toasty/ ./toasty/

USER 1001
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/app"]
