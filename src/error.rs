//! 统一错误类型：所有 handler 返回 `Result<T, AppError>`，
//! 通过 `IntoResponse` 输出统一 JSON 错误结构：
//!
//! ```json
//! { "error": { "code": "not_found", "message": "..." } }
//! ```

use axum::{
    extract::rejection::JsonRejection,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

/// 应用统一错误。
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    /// 404 资源不存在。
    #[error("{0}")]
    NotFound(String),

    /// 400 请求参数/体非法。
    #[error("{0}")]
    BadRequest(String),

    /// 409 资源冲突（如 email 已存在）。
    #[error("{0}")]
    Conflict(String),

    /// 500 内部错误。
    #[error("{0}")]
    Internal(String),
}

impl AppError {
    /// 对应的 HTTP 状态码。
    fn status(&self) -> StatusCode {
        match self {
            Self::NotFound(_) => StatusCode::NOT_FOUND,
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::Conflict(_) => StatusCode::CONFLICT,
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// 机器可读的错误码（用于日志与客户端判断）。
    fn code(&self) -> &'static str {
        match self {
            Self::NotFound(_) => "not_found",
            Self::BadRequest(_) => "bad_request",
            Self::Conflict(_) => "conflict",
            Self::Internal(_) => "internal",
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = self.status();
        let code = self.code();
        let message = self.to_string();

        // 内部错误完整链路进日志（tracing），响应体只给通用信息
        if matches!(self, Self::Internal(_)) {
            tracing::error!(error = %message, "internal error");
        }

        let body = json!({
            "error": { "code": code, "message": message }
        });
        (status, Json(body)).into_response()
    }
}

/// axum JSON 提取失败（请求体非法）→ 400。
impl From<JsonRejection> for AppError {
    fn from(rejection: JsonRejection) -> Self {
        Self::BadRequest(rejection.to_string())
    }
}

/// toasty 数据库错误 → 500（错误详情已在调用处记录，此处统一包装）。
impl From<toasty::Error> for AppError {
    fn from(err: toasty::Error) -> Self {
        Self::Internal(err.to_string())
    }
}
