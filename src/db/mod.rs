//! 数据库连接构建。
//!
//! toasty 的连接池 `Db` 是 Clone 的池句柄（内部连接池复用），
//! axum 中直接放入 `AppState`，每个请求 `state.db.clone()` 得到
//! 自己的可变句柄，互不阻塞。

use crate::config::Config;

/// 构建 toasty 连接池。
///
/// `models!(crate::*)` 注册 crate 内全部模型——server 与 migration CLI
/// 都通过本函数建库，保证迁移工具 diff 的对象与运行时完全一致。
pub async fn build_db(cfg: &Config) -> toasty::Result<toasty::Db> {
    let mut builder = toasty::Db::builder();
    builder.models(toasty::models!(crate::*));
    builder.max_pool_size(cfg.db_pool_max_size as usize);
    builder.pool_pre_ping(true); // 取连接前探活，避免拿到失效连接
    builder.connect(&cfg.database_url).await
}

/// 就绪探针用的轻量数据库探活：`SELECT 1`。
pub async fn ping(db: &mut toasty::Db) -> toasty::Result<()> {
    toasty::sql::statement("SELECT 1").exec(db).await?;
    Ok(())
}
