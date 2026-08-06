//! 迁移 CLI 入口。
//!
//! 与 server 共用同一个 lib（模型、连接构建），保证 schema diff 一致。
//!
//! 用法（在项目根目录，Config::load 读取 Toasty.toml）：
//!
//! ```text
//! # 首次：根据模型生成初始迁移 SQL 到 toasty/migrations/
//! cargo run --bin cli -- migration generate --name initial
//!
//! # 应用迁移（K8s 部署中作为 initContainer 执行）
//! cargo run --bin cli -- migration apply
//! ```

use toasty_cli::{Config, ToastyCli};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = Config::load()?;
    let app_config = axum_k8s_app::config::Config::from_env().map_err(anyhow::Error::msg)?;
    let db = axum_k8s_app::db::build_db(&app_config)
        .await
        .map_err(anyhow::Error::from)?;

    ToastyCli::with_config(db, config)
        .parse_and_run()
        .await
        .map_err(anyhow::Error::from)?;
    Ok(())
}
