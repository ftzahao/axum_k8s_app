//! 存活/就绪探针。
//!
//! - `/healthz`：进程存活，恒返回 200，不依赖任何外部资源
//! - `/readyz`：就绪检查，验证 DB 可达（`SELECT 1`），失败返回 503，
//!   K8s 据此摘除/恢复流量，避免把请求打到连接不上的实例

use axum::{
    extract::State,
    http::StatusCode,
    Json,
};
use serde_json::{json, Value};

use crate::{db, AppState};

/// GET /healthz
pub async fn healthz() -> Json<Value> {
    Json(json!({ "status": "ok" }))
}

/// GET /readyz
pub async fn readyz(State(state): State<AppState>) -> Result<Json<Value>, StatusCode> {
    let mut db = state.db.clone();
    db::ping(&mut db).await.map_err(|err| {
        tracing::error!(error = %err, "readiness check failed");
        StatusCode::SERVICE_UNAVAILABLE
    })?;
    Ok(Json(json!({ "status": "ok", "db": "up" })))
}
