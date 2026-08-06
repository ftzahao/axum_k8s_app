//! 应用入口：初始化日志 → 建连接池 → 启动 HTTP 服务 → 优雅关闭。

use std::net::SocketAddr;

use tokio::signal::unix::{signal, SignalKind};
use tracing_subscriber::EnvFilter;

use axum_k8s_app::{config::Config, db, routes, AppState};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 1. 加载配置（失败即退出，K8s CrashLoopBackOff 兜底）
    let config = Config::from_env().map_err(|e| anyhow::anyhow!(e))?;

    // 2. 初始化日志：JSON 格式输出到 stdout，级别由 RUST_LOG 控制。
    //    不负责持久化——stdout 交给 K8s 的 Fluent Bit 采集。
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::new(&config.rust_log))
        .json()
        .with_target(false)
        .with_current_span(false)
        .init();

    tracing::info!(bind_addr = %config.bind_addr, "starting axum_k8s_app");

    // 3. 建连接池（启动即连接，DB 不可达则启动失败）
    let db = db::build_db(&config).await?;
    tracing::info!("database connection pool ready");

    // 4. 组装路由与监听
    let state = AppState { db };
    let app = routes::build_router(state);

    let addr: SocketAddr = config.bind_addr.parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, "listening");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    tracing::info!("server shut down gracefully");
    Ok(())
}

/// 优雅关闭信号：SIGTERM（K8s 停止 Pod）或 Ctrl-C（本地调试）。
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl-C handler");
    };

    let terminate = async {
        signal(SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    tracing::info!("shutdown signal received");
}
