//! User 模型定义。
//!
//! toasty 中 model 即 schema：`#[derive(toasty::Model)]` 同时在编译期生成
//! 查询构建器（`all()` / `filter_by_*`）、`create!` / `update!` 宏所需的类型，
//! 以及迁移 CLI diff 用的模式快照。

use serde::Serialize;

/// 用户表。
///
/// - `id`: 自增主键（Postgres identity 列）
/// - `email`: 唯一约束，重复创建返回 409
/// - `created_at`: 字段名为 `created_at` 时，`#[auto]` 展开为
///   `#[default(jiff::Timestamp::now())]`——create 时自动填充，无需 handler 赋值
#[derive(Debug, Clone, Serialize, toasty::Model)]
pub struct User {
    #[key]
    #[auto]
    pub id: u64,

    pub name: String,

    #[unique]
    pub email: String,

    #[auto]
    pub created_at: jiff::Timestamp,
}
