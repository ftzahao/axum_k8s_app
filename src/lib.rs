//! 共享库：server（main.rs）与迁移 CLI（bin/cli.rs）共用
//! 同一份模型、配置与连接构建，保证迁移 diff 的对象与运行时一致。

pub mod config;
pub mod db;
pub mod error;
pub mod handlers;
pub mod middleware;
pub mod models;
pub mod routes;

/// 全局共享状态。
///
/// `db` 是 toasty 连接池的 Clone 句柄，放入 axum State 后
/// 每个请求 `state.db.clone()` 拿到独立的可变句柄，互不阻塞。
#[derive(Clone)]
pub struct AppState {
    pub db: toasty::Db,
}
