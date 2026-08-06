//! 路由组装。

use axum::{
    routing::get,
    Router,
};

use crate::{
    handlers::{health, users},
    middleware::request_log,
    AppState,
};

/// 构建应用路由。
///
/// 探针路由（/healthz /readyz）先挂载且不经过请求日志中间件——
/// K8s 默认 3s 探测一次，打日志只会刷屏。
pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(health::healthz))
        .route("/readyz", get(health::readyz))
        .route(
            "/api/users",
            get(users::list_users).post(users::create_user),
        )
        .route(
            "/api/users/{id}",
            get(users::get_user)
                .put(users::update_user)
                .delete(users::delete_user),
        )
        .layer(axum::middleware::from_fn(request_log::request_log))
        .with_state(state)
}
