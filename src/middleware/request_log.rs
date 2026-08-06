//! 请求日志中间件：每个请求一行结构化日志，
//! 字段包含 method、path、status、latency_ms。

use std::time::Instant;

use axum::{
    body::Body,
    http::Request,
    middleware::Next,
    response::Response,
};
use tracing::{info_span, Instrument};

/// 请求日志中间件。
///
/// - 请求进入时开启 span（method/path 作为 span 属性，贯穿请求内所有日志）
/// - 响应返回时输出一行完成日志（status、latency_ms）
pub async fn request_log(req: Request<Body>, next: Next) -> Response {
    let method = req.method().clone();
    let path = req.uri().path().to_string();

    let span = info_span!("request", method = %method, path = %path);

    async move {
        let start = Instant::now();
        let resp = next.run(req).await;
        let latency_ms = start.elapsed().as_millis() as u64;

        tracing::info!(
            status = resp.status().as_u16(),
            latency_ms,
            "request completed"
        );
        resp
    }
    .instrument(span)
    .await
}
