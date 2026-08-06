# axum_k8s_app

生产级 Rust 后端示例：Axum 0.8 + Toasty ORM + PostgreSQL，部署到 Kubernetes。

## 技术栈

| 组件 | 说明 |
|---|---|
| Web | Axum 0.8 |
| ORM | toasty 0.9（tokio-rs，`#[derive(Model)]` 定义 schema） |
| 数据库 | PostgreSQL（toasty `postgresql` feature，连接池内置于 `Db`） |
| 日志 | tracing + tracing-subscriber，JSON 行输出 stdout，`RUST_LOG` 控制级别 |
| 迁移 | toasty-cli 项目内 CLI：`migration generate / apply`，K8s 中由 initContainer 自动执行 |
| 部署 | 多阶段 Dockerfile（debian:bookworm-slim + 非 root）→ Deployment/Service/Ingress + Fluent Bit → Loki → Grafana |

## 项目结构

```
src/
├── main.rs          入口：tracing 初始化、建池、路由、优雅关闭（SIGTERM/SIGINT）
├── lib.rs           共享库（server 与迁移 CLI 共用同一份模型/连接）
├── config.rs        环境变量配置（BIND_ADDR / DATABASE_URL / RUST_LOG / DB_POOL_MAX_SIZE）
├── error.rs         AppError 统一错误 → JSON（400/404/409/500）
├── routes.rs        路由组装（探针路由不经过请求日志中间件）
├── db/              build_db()（连接池）+ ping()（readyz 探活）
├── models/          User 模型（toasty::Model）
├── handlers/        health（/healthz /readyz）、users（CRUD）
├── middleware/      请求日志中间件（method/path/status/latency_ms）
└── bin/cli.rs       迁移 CLI（toasty_cli）
toasty/              迁移产物（history.toml + SQL + 快照，提交进 git）
k8s/                 全部 K8s 清单与部署手册
```

## 本地开发

```bash
# 1. 起本地 PostgreSQL（macOS：Apple container 或 docker）
container run -d --name pg-dev \
  -e POSTGRES_USER=app -e POSTGRES_PASSWORD=dev-password -e POSTGRES_DB=app \
  -p 5432:5432 docker.io/library/postgres:18

# 2. 生成迁移（改过模型后执行；首次运行）
DATABASE_URL='postgresql://app:dev-password@localhost:5432/app' \
  cargo run --bin cli -- migration generate --name initial

# 3. 应用迁移
DATABASE_URL='postgresql://app:dev-password@localhost:5432/app' \
  cargo run --bin cli -- migration apply

# 4. 启动服务
DATABASE_URL='postgresql://app:dev-password@localhost:5432/app' \
  RUST_LOG=debug cargo run
```

## 接口

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | /healthz | 存活探针 |
| GET | /readyz | 就绪探针（DB `SELECT 1`） |
| GET/POST | /api/users | 列表 / 创建 |
| GET/PUT/DELETE | /api/users/{id} | 详情 / 更新 / 删除 |

错误响应统一格式：`{"error":{"code":"not_found|bad_request|conflict|internal","message":"..."}}`

## 构建与部署

完整安装部署文档见 [INSTALL.md](INSTALL.md)（本地环境 → 构建镜像 → K8s 部署 → 日志链路 → 升级回滚 → 故障排查）。
集群操作速查见 [k8s/README.md](k8s/README.md)。

## 关键设计点

- **迁移与运行时模型一致**：`bin/cli.rs` 与 server 共用 `src/lib.rs` 的 `build_db()` 与模型定义，`toasty::models!(crate::*)` 注册全部模型。
- **`created_at` 自动填充**：`#[auto]` 在名为 `created_at` 的字段上展开为 `#[default(jiff::Timestamp::now())]`。
- **时间类型**：toasty 使用 `jiff`（0.2，与 toasty 内部对齐），映射 Postgres `TIMESTAMPTZ`。
- **探针与日志分离**：探针路由不经过请求日志中间件，避免 K8s 3s 一次的探测刷屏。
- **优雅关闭双保险**：K8s `preStop sleep 10`（Ingress 摘流）+ 应用内 `with_graceful_shutdown`（排空在途请求），`terminationGracePeriodSeconds: 35` 覆盖两者。
- **应用不持久化日志**：JSON 行写 stdout，Fluent Bit 采集后送 Loki（Grafana 查询）。
