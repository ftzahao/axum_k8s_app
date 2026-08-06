//! 用户 CRUD handler。
//!
//! 统一模式：
//! - 每个 handler `state.db.clone()` 得到自己的 `&mut` 池句柄，互不阻塞
//! - 404 判断用 `first().exec()` 拿 `Option`，不依赖 toasty 错误变体
//! - email 唯一冲突在应用层预检（409），数据库唯一约束兜底并发竞态

use axum::{
    extract::{Path, State},
    Json,
};
use serde::Deserialize;

use crate::{
    error::AppError,
    models::User,
    AppState,
};

/// POST /api/users 请求体
#[derive(Debug, Deserialize)]
pub struct CreateUser {
    name: String,
    email: String,
}

/// PUT /api/users/{id} 请求体
#[derive(Debug, Deserialize)]
pub struct UpdateUser {
    name: String,
    email: String,
}

/// GET /api/users —— 用户列表
pub async fn list_users(State(state): State<AppState>) -> Result<Json<Vec<User>>, AppError> {
    let mut db = state.db.clone();
    let users = User::all().exec(&mut db).await?;
    Ok(Json(users))
}

/// POST /api/users —— 创建用户
pub async fn create_user(
    State(state): State<AppState>,
    Json(payload): Json<CreateUser>,
) -> Result<Json<User>, AppError> {
    let name = payload.name.trim().to_string();
    let email = payload.email.trim().to_string();

    if name.is_empty() {
        return Err(AppError::BadRequest("name 不能为空".into()));
    }
    if !email.contains('@') {
        return Err(AppError::BadRequest("email 格式不正确".into()));
    }

    let mut db = state.db.clone();

    // email 唯一预检 → 409（并发竞态由数据库唯一约束兜底）
    if User::filter_by_email(&email).first().exec(&mut db).await?.is_some() {
        return Err(AppError::Conflict(format!("email `{email}` 已存在")));
    }

    // created_at 由 #[auto]（#[default(jiff::Timestamp::now())]）自动填充
    let user = toasty::create!(User {
        name: name.clone(),
        email: email.clone(),
    })
    .exec(&mut db)
    .await?;

    tracing::info!(user_id = user.id, email = %user.email, "user created");
    Ok(Json(user))
}

/// GET /api/users/{id} —— 用户详情
pub async fn get_user(
    State(state): State<AppState>,
    Path(id): Path<u64>,
) -> Result<Json<User>, AppError> {
    let mut db = state.db.clone();
    let user = User::filter_by_id(id)
        .first()
        .exec(&mut db)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("用户 {id} 不存在")))?;
    Ok(Json(user))
}

/// PUT /api/users/{id} —— 更新用户
pub async fn update_user(
    State(state): State<AppState>,
    Path(id): Path<u64>,
    Json(payload): Json<UpdateUser>,
) -> Result<Json<User>, AppError> {
    let name = payload.name.trim().to_string();
    let email = payload.email.trim().to_string();

    if name.is_empty() {
        return Err(AppError::BadRequest("name 不能为空".into()));
    }
    if !email.contains('@') {
        return Err(AppError::BadRequest("email 格式不正确".into()));
    }

    let mut db = state.db.clone();

    // 目标必须存在 → 404
    let _existing = User::filter_by_id(id)
        .first()
        .exec(&mut db)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("用户 {id} 不存在")))?;

    // email 冲突预检（排除自身）→ 409
    if let Some(dup) = User::filter_by_email(&email).first().exec(&mut db).await? {
        if dup.id != id {
            return Err(AppError::Conflict(format!("email `{email}` 已被其他用户使用")));
        }
    }

    // update! 宏：target 为查询构建器表达式，按 id 定位记录，更新字段
    toasty::update!(User::filter_by_id(id) {
        name: name.clone(),
        email: email.clone(),
    })
    .exec(&mut db)
    .await?;

    let updated = User::filter_by_id(id)
        .first()
        .exec(&mut db)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("用户 {id} 不存在")))?;

    tracing::info!(user_id = updated.id, "user updated");
    Ok(Json(updated))
}

/// DELETE /api/users/{id} —— 删除用户
pub async fn delete_user(
    State(state): State<AppState>,
    Path(id): Path<u64>,
) -> Result<axum::http::StatusCode, AppError> {
    let mut db = state.db.clone();

    // 目标必须存在 → 404
    User::filter_by_id(id)
        .first()
        .exec(&mut db)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("用户 {id} 不存在")))?;

    // Query 转 Delete 语句
    // 验证项：若 filter_by_id 返回类型无 .delete()，改用实例方法 user.delete()
    User::filter_by_id(id).delete().exec(&mut db).await?;

    tracing::info!(user_id = id, "user deleted");
    Ok(axum::http::StatusCode::NO_CONTENT)
}
