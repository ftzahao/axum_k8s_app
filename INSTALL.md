# 安装与部署文档

本文档覆盖从零到可用的完整安装流程：本地开发环境 → 构建镜像 → Kubernetes 部署 → 日志链路（Fluent Bit → Loki → Grafana）→ 升级与回滚。

已验证版本组合（2026-08）：

| 组件 | 版本 | 说明 |
|---|---|---|
| Rust | 1.85+（edition 2024 要求） | `rustc --version` 确认 |
| axum | 0.8.x | crates.io 最新稳定 |
| toasty / toasty-cli | 0.9.x | 两个 crate 必须同版本号 |
| PostgreSQL | 18.x | 当前最新稳定版（19 未发布） |
| Fluent Bit | 3.x | DaemonSet 采集节点容器日志 |
| Loki / Grafana | Helm chart 最新 | `grafana/loki` + `grafana/grafana` |

---

## 1. 架构与部署拓扑

```
                    ┌─────────────────────────────────────────────┐
  客户端            │ Kubernetes 集群                              │
  api.example.com   │                                             │
      │             │  ┌────────────┐   ┌─────────────────────┐   │
      ▼             │  │  Ingress   │──▶│ Service (ClusterIP) │   │
 ┌─────────┐        │  │  nginx     │   └─────────┬───────────┘   │
 │  DNS    │        │  └────────────┘             │               │
 └─────────┘        │              ┌──────────────┼───────────┐   │
                    │              ▼              ▼           ▼   │
                    │     ┌───────────────── Deployment ──────────┐│
                    │     │ initContainer: cli migration apply    ││
                    │     │ app × 3（探针/优雅关闭/资源限制）        ││
                    │     └───────────┬───────────────────────────┘│
                    │                 │                            │
                    │      ┌──────────▼──────────┐                 │
                    │      │ PostgreSQL（外部/托管）│                │
                    │      └─────────────────────┘                 │
                    │                                             │
                    │  ┌──────────────── 日志链路 ────────────────┐ │
                    │  │ stdout JSON ─▶ Fluent Bit DaemonSet ─▶  │ │
                    │  │ Loki ─▶ Grafana（Helm 安装）              │ │
                    │  └─────────────────────────────────────────┘ │
                    └─────────────────────────────────────────────┘
```

关键机制：

- **迁移在启动时自动执行**：Pod 的 initContainer 运行 `cli migration apply`（镜像内置迁移文件），成功后才启动主容器。迁移失败会阻断发布。
- **探针分层**：`/healthz`（存活，失败重启容器）与 `/readyz`（就绪，失败摘流量不重启）。
- **优雅关闭**：`preStop sleep 10` 让 Ingress 摘流 → 应用收到 SIGTERM 后 `with_graceful_shutdown` 排空在途请求，`terminationGracePeriodSeconds: 35` 兜底。
- **日志链路**：应用输出 JSON 行到 stdout → containerd 记录为 CRI 格式 → Fluent Bit 解析并附加 Pod 标签 → Loki → Grafana 查询。应用不持久化日志。

---

## 2. 前置条件

### 2.1 开发机

| 工具 | 版本要求 | 验证命令 |
|---|---|---|
| Rust toolchain | 1.85+ | `rustc --version` |
| Cargo | 随 Rust | `cargo --version` |
| 容器运行时 | 任一 | `container --version`（Apple container）或 `docker --version` |

### 2.2 集群侧

| 组件 | 用途 | 验证命令 |
|---|---|---|
| kubectl | 管理集群 | `kubectl version --client` |
| Helm | 安装 Loki/Grafana/ingress-nginx | `helm version` |
| ingress-nginx | Ingress 控制器（域名入口） | `kubectl get ingressclass nginx` |
| PostgreSQL 18 | 应用数据库（外部/托管/集群内均可） | 应用连接串可用 |

> 没有现成集群？可用 kind / minikube / k3s 起本地测试集群（见第 6 节）。

---

## 3. 本地开发环境（最快路径）

### 3.1 启动 PostgreSQL 18

macOS（Apple container）：

```bash
container system start   # 首次需要
container run -d --name pg-dev \
  -e POSTGRES_USER=app \
  -e POSTGRES_PASSWORD=dev-password \
  -e POSTGRES_DB=app \
  -p 5432:5432 \
  docker.io/library/postgres:18
```

Docker 等价命令：

```bash
docker run -d --name pg-dev \
  -e POSTGRES_USER=app \
  -e POSTGRES_PASSWORD=dev-password \
  -e POSTGRES_DB=app \
  -p 5432:5432 \
  postgres:18
```

等待就绪：

```bash
container logs pg-dev | grep 'ready to accept connections'   # Apple container
docker logs pg-dev | grep 'ready to accept connections'      # Docker
```

### 3.2 生成并应用迁移

```bash
cd axum_k8s_app
export DATABASE_URL='postgresql://app:dev-password@localhost:5432/app'

# 首次：根据 src/models/ 生成初始迁移（产出 toasty/migrations/*.sql + 快照）
cargo run --bin cli -- migration generate --name initial

# 应用迁移（记录到数据库 __toasty_migrations 表，幂等）
cargo run --bin cli -- migration apply
```

> 修改过 `src/models/` 后再次执行 `migration generate --name xxx` 生成增量迁移，**toasty/ 目录需提交进 git**——镜像构建与集群部署都依赖它。

### 3.3 启动服务并验证

```bash
cargo run   # RUST_LOG 默认 info；可加 RUST_LOG=debug
```

```bash
curl -s http://localhost:8080/healthz                # {"status":"ok"}
curl -s http://localhost:8080/readyz                 # {"db":"up","status":"ok"}
curl -s -X POST http://localhost:8080/api/users \
  -H 'Content-Type: application/json' \
  -d '{"name":"Alice","email":"alice@example.com"}'
# {"id":1,"name":"Alice","email":"alice@example.com","created_at":"..."}
```

日志为 JSON 行（含请求级 method/path/status/latency_ms），停止用 Ctrl-C（走优雅关闭）。

### 3.4 清理本地环境

```bash
container stop pg-dev && container delete pg-dev      # Apple container
docker stop pg-dev && docker rm pg-dev                # Docker
```

---

## 4. 构建与推送镜像

```bash
docker build -t <registry>/axum-k8s-app:<tag> .
docker push <registry>/axum-k8s-app:<tag>
```

镜像内容：

| 内容 | 用途 |
|---|---|
| `/usr/local/bin/app` | 服务二进制（ENTRYPOINT） |
| `/usr/local/bin/cli` | 迁移 CLI（initContainer 执行 `cli migration apply`） |
| `/app/Toasty.toml` + `/app/toasty/` | 迁移配置与 SQL 文件（`Config::load()` 从工作目录读取） |

构建说明：

- **多阶段构建**：builder（rust:1-bookworm）→ runtime（debian:bookworm-slim，非 root 用户 `app`）。
- **依赖缓存层**：先以占位 main 构建一次缓存全部依赖，业务代码改动不重编依赖。
- 使用 `--locked`：要求 Cargo.lock 与 Cargo.toml 匹配（提交两者到 git）。

---

## 5. 部署到 Kubernetes

### 5.1 准备 PostgreSQL

应用假定 `DATABASE_URL` 指向可用库。生产建议用托管实例（RDS/Cloud SQL 等）；测试可临时用集群内单点（仅限测试）：

```bash
kubectl -n app run pg-test --image=postgres:18 --restart=Never --port=5432 \
  --env='POSTGRES_USER=app' --env='POSTGRES_PASSWORD=password' \
  --env='POSTGRES_DB=app'
kubectl -n app expose pod pg-test --port=5432 --name pg-test
# DATABASE_URL=postgresql://app:password@pg-test:5432/app
```

### 5.2 安装 ingress-nginx（如未安装）

```bash
helm upgrade --install ingress-nginx ingress-nginx \
  --repo https://kubernetes.github.io/ingress-nginx \
  --namespace ingress-nginx --create-namespace
kubectl wait --namespace ingress-nginx \
  --for=condition=ready pod --selector=app.kubernetes.io/component=controller \
  --timeout=120s
```

### 5.3 安装 Loki + Grafana（Helm）

```bash
helm repo add grafana https://grafana.github.io/helm-charts
helm repo update

helm install loki grafana/loki \
  --namespace monitoring --create-namespace
# 可选：单实例模式限制副本（默认即可）
#   --set singleBinary.replicas=1

helm install grafana grafana/grafana \
  --namespace monitoring \
  --set persistence.enabled=true
```

> 说明：本仓库自带 Fluent Bit DaemonSet（`k8s/fluent-bit.yaml`），故不安装 loki-stack（避免其内嵌 promtail/fluent-bit 重复采集）。Loki 服务名固定为 `loki.monitoring:3100`（取决于 helm release 名 `loki`）。
> Loki 默认带 PVC；生产环境建议按存储规模配置对象存储/持久卷。

### 5.4 应用部署清单（按顺序）

```bash
kubectl apply -f k8s/namespace.yaml

# Secret：用真实连接串生成（值不进 git）
kubectl create secret generic app-secret -n app \
  --from-literal=DATABASE_URL='postgresql://user:password@pg-host:5432/app' \
  --dry-run=client -o yaml | kubectl apply -f -

kubectl apply -f k8s/configmap.yaml        # RUST_LOG / BIND_ADDR / DB_POOL_MAX_SIZE
kubectl apply -f k8s/deployment.yaml       # 替换镜像 tag 后执行
kubectl apply -f k8s/service.yaml
kubectl apply -f k8s/ingress.yaml          # 域名 api.example.com
kubectl apply -f k8s/fluent-bit.yaml       # 日志采集（monitoring 命名空间）
```

> 部署前编辑 [k8s/deployment.yaml](k8s/deployment.yaml)，将两处 `image: axum-k8s-app:latest` 替换为第 4 节推送的镜像地址。

### 5.5 验证部署

```bash
# 等待 3 个副本全部就绪（initContainer 先完成迁移）
kubectl -n app get pods -w

# 探针
kubectl -n app exec deploy/app -- curl -s http://localhost:8080/healthz
kubectl -n app exec deploy/app -- curl -s http://localhost:8080/readyz

# 业务接口（DNS 解析到集群后，或先 port-forward 验证）
kubectl -n app port-forward svc/app 8080:8080 &
curl -s http://localhost:8080/api/users

# 域名
curl -s https://api.example.com/healthz
```

### 5.6 验证日志链路

```bash
kubectl -n monitoring get pods          # fluent-bit DaemonSet 每节点一个
kubectl -n monitoring logs ds/fluent-bit
kubectl -n app logs deploy/app          # 应用 JSON 日志

# 查看 Grafana 地址并登录（默认 admin/admin，首次修改）
kubectl -n monitoring get svc grafana
kubectl -n monitoring port-forward svc/grafana 3000:80
```

Grafana 配置数据源：**Connections → Data sources → Add data source → Loki**，URL 填 `http://loki.monitoring:3100`。

查询示例（应用日志为结构化 JSON，可直接按字段过滤）：

```logql
{namespace="app"} | json | level = "error"
{namespace="app"} | json | status > 500
{namespace="app"} | json | latency_ms > 100
```

---

## 6. 本地测试集群（kind 示例）

无现成集群时：

```bash
# 安装 kind
brew install kind
kind create cluster --name dev

# 加载镜像到 kind（比 push registry 快）
kind load docker-image axum-k8s-app:latest

# 安装 ingress-nginx（kind 需要特殊配置：nodeport/ hostPort）
helm upgrade --install ingress-nginx ingress-nginx \
  --repo https://kubernetes.github.io/ingress-nginx \
  --namespace ingress-nginx --create-namespace \
  --set controller.hostPort.enabled=true \
  --set controller.service.type=NodePort

# 之后按第 5 节步骤部署；域名验证改 /etc/hosts：
# 127.0.0.1 api.example.com
```

---

## 7. 生产配置要点

| 项 | 建议 | 位置 |
|---|---|---|
| Secret 管理 | SealedSecret / External Secrets Operator / Vault，禁止明文 secret.yaml 进仓库 | k8s/secret.yaml |
| TLS | cert-manager + ClusterIssuer（Let's Encrypt），Ingress 加 `tls:` 段 | k8s/ingress.yaml |
| 连接池 | 3 副本 × 10 = 30 连接，与 PG `max_connections` 核对 | ConfigMap `DB_POOL_MAX_SIZE` |
| 资源配额 | 按压测结果调 k8s/deployment.yaml 的 requests/limits | k8s/deployment.yaml |
| 日志级别 | 生产建议 `RUST_LOG=info`；排障临时改 debug 后滚动重启 | ConfigMap |
| 滚动更新 | `maxUnavailable: 1 / maxSurge: 1` 已配置（逐个替换，不停服） | k8s/deployment.yaml |

---

## 8. 升级与回滚

### 8.1 应用代码升级

```bash
# 1. 本地修改代码/模型
# 2. 模型变更时生成增量迁移（必须）
DATABASE_URL='postgresql://...' cargo run --bin cli -- migration generate --name add_xxx
git add toasty/ && git commit          # toasty/ 必须提交

# 3. 构建推送新镜像
docker build -t <registry>/axum-k8s-app:<new-tag> .
docker push <registry>/axum-k8s-app:<new-tag>

# 4. 滚动发布（新 Pod 的 initContainer 自动 apply 增量迁移）
kubectl -n app set image deploy/app app=<registry>/axum-k8s-app:<new-tag>
kubectl -n app rollout status deploy/app
```

### 8.2 代码回滚（K8s 层）

**先决条件：镜像必须使用不可变 tag**（如 commit SHA 或日期号，`axum-k8s-app:<git-sha>`）。如果一直用 `:latest`，`rollout undo` 换回的"旧版本"实际上还会拉取同一个 `:latest`，回滚无效。推荐用 tag 的镜像发布流程：

```bash
IMAGE=<registry>/axum-k8s-app:$(git rev-parse --short HEAD)
docker build -t $IMAGE . && docker push $IMAGE
kubectl -n app set image deploy/app app=$IMAGE
```

回滚到上一个版本的完整操作：

```bash
# 1. 查看发布历史（rev 编号对应各版本）
kubectl -n app rollout history deploy/app

# 2. 回滚到上一个版本
kubectl -n app rollout undo deploy/app

# 或回滚到指定版本
kubectl -n app rollout undo deploy/app --to-revision=<rev号>

# 3. 等待回滚完成并验证
kubectl -n app rollout status deploy/app
kubectl -n app get pods
kubectl -n app logs deploy/app | tail -20
```

**回滚过程中实际发生了什么**（3 个副本逐个替换）：

1. 新 Pod 启动 → initContainer 执行 `cli migration apply`
2. 迁移 apply 是**幂等**的：数据库 `__toasty_migrations` 表记录了已应用的迁移，旧镜像中已执行过的迁移会被跳过，不会重复执行
3. 主容器就绪后接入流量，旧 Pod 走优雅关闭退出

> 注意：回滚**不会**自动把镜像切回 initContainer 的迁移文件来源——initContainer 用的是回滚后镜像自带的 `toasty/` 目录（即旧版本的迁移历史），这正好保证 apply 与旧代码一致。

### 8.3 数据库 schema 不会随代码回滚

toasty 迁移是**前向**的：一旦某个迁移被 `apply`，它就被数据库视为"已执行"，永远不会被自动撤销。`rollout undo` 只换回旧代码，数据库结构停留在最新迁移后的状态。

两种回滚场景的影响完全不同：

| 场景 | 迁移内容 | 回滚到旧代码后 |
|---|---|---|
| 无害 | 加表、加列、加索引（不删除不改动现有结构） | 正常运行；新增的表/列闲置 |
| **危险** | 删列、删表、改类型、重命名 | 旧代码仍查询这些列/类型 → 接口 500 |

**示例**：v2 迁移 `ALTER TABLE users DROP COLUMN phone`，v2 代码也停用了 `phone`。回滚到 v1（代码仍 `SELECT phone`）→ 数据库已无该列 → 所有用户查询报错。回滚代码后**数据库没有"撤销"这次 DROP**，v1 代码永远无法恢复。

### 8.4 迁移设计原则：让回滚始终安全

核心原则：**迁移只做向后兼容的变更，破坏性操作延后两个版本**（expand-contract 两阶段法）。

| 阶段 | 代码版本 | 迁移 | 状态 |
|---|---|---|---|
| expand | v1 | 加新列 `nickname`（旧代码不使用） | 表结构兼容 |
| 切换 | v2 | 代码改用 `nickname` | 可回滚到 v1 |
| contract | v3 | 删旧列 `name` | 只能前进，不能回滚到 v2 之前 |

即：

- **加**（列/表/索引）：随时可以做，永远安全
- **改**（类型/默认值）：避免；必须改时评估旧代码读取兼容性
- **重命名**：不加 `RENAME COLUMN`，改为"加新列 + 代码切换 + 下个版本删旧列"
- **删除**：只在确认不会回滚到使用它的版本之后再删

回滚代码前自查清单：

```text
□ 自上次 apply 以来，有没有 删列/删表/改类型/重命名 的迁移？
□ 如果有，要回滚到的旧代码是否仍在读取这些对象？
□ 是 → 不能直接回滚，先手动恢复 schema（见 8.5）或放弃回滚改走"修复代码"路线
```

### 8.5 必须撤销 schema 变更时（罕见）

仅当迁移确实破坏了旧代码且必须回滚时，才考虑手动撤销。**维护窗口内**人工执行反向 SQL：

```sql
-- 示例：v2 删了 users.phone，现在要恢复
ALTER TABLE users ADD COLUMN phone TEXT;
```

**绝不要在生产用以下命令**：

| 命令 | 实际行为 | 为什么危险 |
|---|---|---|
| `cli migration drop` | 仅把迁移从 `history.toml` 移除，**不碰数据库** | 下次 `apply` 会认为该迁移从未执行而重复执行 → 报错冲突 |
| `cli migration reset` | 删除**所有**表并重放迁移 | 数据全部丢失，仅限开发环境 |

这两个命令是开发工具。生产 schema 撤销 = 人工编写反向 SQL（如上面的 `ADD COLUMN`）并在执行后更新代码或再发布修复版本。

---

## 9. 故障排查

| 症状 | 排查 |
|---|---|
| Pod 卡在 `Init:CrashLoopBackOff` | `kubectl -n app logs deploy/app -c migrate`——迁移 SQL 报错 |
| Pod `CrashLoopBackOff` | `kubectl -n app logs deploy/app`——通常是 DATABASE_URL 错误或不可达 |
| readiness 失败（Pod Running 但不 Ready） | `kubectl -n app logs deploy/app | grep readiness`——DB 连接问题或 PG `max_connections` 打满 |
| 接口返回 503/超时 | 检查 Service 端点：`kubectl -n app get endpoints app` |
| 域名不通 | `kubectl -n app get ingress`；确认 DNS 指向 ingress-nginx 的 LoadBalancer IP；`kubectl -n ingress-nginx get svc` |
| 创建用户 409 | 预期行为：email 唯一约束冲突（应用层预检 + 数据库兜底） |
| 日志未进 Loki | `kubectl -n monitoring logs ds/fluent-bit`；确认 Loki service 名与 `fluent-bit.conf` 的 `Host loki / Port 3100` 一致 |
| Grafana 查不到日志 | 检查 datasource URL（`http://loki.monitoring:3100`）；用 `{namespace="app"}` 简化查询先确认数据存在 |

---

## 10. 附录

### 10.1 环境变量

| 变量 | 必填 | 默认 | 说明 |
|---|---|---|---|
| `DATABASE_URL` | ✅ | — | PostgreSQL 连接串，如 `postgresql://user:pass@host:5432/db` |
| `BIND_ADDR` | | `0.0.0.0:8080` | HTTP 监听地址 |
| `RUST_LOG` | | `info` | tracing 级别（error/warn/info/debug/trace） |
| `DB_POOL_MAX_SIZE` | | `10` | 连接池上限（u32） |

### 10.2 迁移 CLI 命令

```bash
cargo run --bin cli -- migration generate --name <name>   # 模型变更 → 增量迁移 SQL
cargo run --bin cli -- migration apply                    # 应用待执行迁移（幂等）
cargo run --bin cli -- migration snapshot                 # 打印当前 schema 快照
cargo run --bin cli -- migration drop                     # 从历史移除迁移（开发工具）
cargo run --bin cli -- migration reset                    # 删表并可选重放（开发工具）
```

### 10.3 接口一览

| 方法 | 路径 | 说明 | 状态码 |
|---|---|---|---|
| GET | `/healthz` | 存活探针 | 200 |
| GET | `/readyz` | 就绪探针（DB 探活） | 200 / 503 |
| GET | `/api/users` | 用户列表 | 200 |
| POST | `/api/users` | 创建用户 `{"name","email"}` | 200 / 400 / 409 |
| GET | `/api/users/{id}` | 用户详情 | 200 / 404 |
| PUT | `/api/users/{id}` | 更新用户 | 200 / 400 / 404 / 409 |
| DELETE | `/api/users/{id}` | 删除用户 | 204 / 404 |

错误统一格式：`{"error":{"code":"not_found|bad_request|conflict|internal","message":"..."}}`
