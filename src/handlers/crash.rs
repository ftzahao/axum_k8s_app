//! 模拟崩溃：供 K8s 存活探针 / 自动重启测试。

use axum::http::StatusCode;

/// GET /unusual
///
/// 立即终止进程（SIGABRT）。
///
/// 注意：axum handler 运行在 tokio 任务里，直接 `panic!` 只会杀死该
/// 请求任务（连接被重置、进程存活、探针照常通过），K8s 不会重启容器。
/// 要让 livenessProbe 失败并触发重启，必须让整个进程死亡——
/// 这里用 `std::process::abort()`（SIGABRT，kubelet 会重启容器）。
pub async fn unusual() -> StatusCode {
    tracing::error!("simulated crash triggered via /unusual");
    // 留 100ms 让日志刷到 stdout 再死
    std::thread::sleep(std::time::Duration::from_millis(100));
    std::process::abort()
}
