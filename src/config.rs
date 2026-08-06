//! 环境变量配置。
//!
//! 生产部署（Kubernetes）中所有配置通过 ConfigMap / Secret 注入环境变量，
//! 因此这里直接手写环境变量解析，不引入额外配置依赖。

use std::env;

/// 应用配置。
#[derive(Debug, Clone)]
pub struct Config {
    /// 监听地址，如 `0.0.0.0:8080`。
    pub bind_addr: String,
    /// PostgreSQL 连接串，如 `postgresql://user:pass@host:5432/db`。
    pub database_url: String,
    /// 连接池最大连接数。
    pub db_pool_max_size: u32,
    /// tracing 日志级别过滤（RUST_LOG）。
    pub rust_log: String,
}

impl Config {
    /// 从环境变量加载配置；缺失必填项时返回错误。
    pub fn from_env() -> Result<Self, String> {
        let bind_addr = env::var("BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".to_string());
        let database_url =
            env::var("DATABASE_URL").map_err(|_| "DATABASE_URL 环境变量未设置".to_string())?;
        let db_pool_max_size = env::var("DB_POOL_MAX_SIZE")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(10);
        let rust_log = env::var("RUST_LOG").unwrap_or_else(|_| "info".to_string());

        Ok(Self {
            bind_addr,
            database_url,
            db_pool_max_size,
            rust_log,
        })
    }
}
