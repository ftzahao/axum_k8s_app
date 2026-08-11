# axum_k8s_app

生产级 Rust 后端示例：Axum 0.8 + Toasty ORM + PostgreSQL，可部署到 Kubernetes。

**本文档即完整教程**：本地开发 → 构建镜像 → K8s 部署 → 环境区分 → 故障排查，一份文件讲完。
（集群操作速查见 [k8s/README.md](k8s/README.md)。）

## 技术栈

| 组件   | 说明 |
| ------ | ---- |
| Web    | Axum 0.8 |
| ORM    | toasty 0.9（`#[derive(toasty::Model)]` 定义 schema） |
| 数据库 | PostgreSQL（toasty `postgresql` feature，连接池内置于 `Db`） |
| 日志   | tracing + tracing-subscriber，JSON 行输出 stdout，`RUST_LOG` 控制级别 |
| 迁移   | toasty-cli 项目内 CLI：`migration generate / apply`，K8s 中由 initContainer 自动执行 |
| 部署   | 多阶段 Dockerfile（alpine + musl 静态二进制 + 非 root）→ Deployment（initContainer 自动 apply 迁移 + 主容器）+ Service/Ingress + Fluent Bit → Loki → Grafana |

项目结构：`src/`（`main.rs` 入口 + `lib.rs` 共用库：config / db / error / routes / models / handlers / middleware / bin/cli）+ `toasty/`（迁移产物：`migrations/` SQL + `history.toml` + `snapshots/`，需提交 git）+ `k8s/`（部署清单）。

---

## 一、本地开发

### 方式 A：原生进程（推荐）

容器只跑 PostgreSQL，应用用 cargo 直跑——改代码即改即生效、日志直接看、Ctrl-C 优雅退出。

```bash
# 1. 起 PostgreSQL（两种运行时任选其一）

# Apple container
container run -d --name pg-dev \
  -e POSTGRES_USER=app -e POSTGRES_PASSWORD=dev-password -e POSTGRES_DB=app \
  -p 5432:5432 docker.io/library/postgres:18

# Docker 等价命令
docker run -d --name pg-dev \
  -e POSTGRES_USER=app -e POSTGRES_PASSWORD=dev-password -e POSTGRES_DB=app \
  -p 5432:5432 postgres:18

# 等待就绪：container logs pg-dev | grep 'ready to accept connections'（Docker 用 docker logs）

# 2. 应用迁移（改过 src/models/ 后先 migration generate 再 apply；toasty/ 需提交 git）
# Apple container 下连 PG 容器不要用 localhost（见下方⚠️），用 PG 容器的 VM IP
export DATABASE_URL="postgresql://app:dev-password@localhost:5432/app"
cargo run --bin cli -- migration apply

# 3. 启动服务（仓库有两个 binary，须用 --bin 指定）
RUST_LOG=debug cargo run --bin axum_k8s_app

# 4. 验证
curl -s http://localhost:8080/healthz                # {"status":"ok"}
curl -s http://localhost:8080/readyz                 # {"db":"up","status":"ok"}
curl -s -X POST http://localhost:8080/api/users \
  -H 'Content-Type: application/json' -d '{"name":"Alice","email":"alice@example.com"}'
```

> ⚠️ **Apple container 宿主端口转发异常（本机实测）**：`-p` 映射的端口在 Apple container 下行为不稳定——实测报 `No route to host (os error 65)` 或 `Connection reset by peer`（不同进程表现不同）。最稳的做法：连接串**不要用 `localhost`**，改用容器 VM IP：`container inspect pg-dev` 查 `ipv4Address`（如 `192.168.64.2`）。详见「六、故障排查」。

清理：`container stop pg-dev && container delete pg-dev`（Docker：`docker stop pg-dev && docker rm pg-dev`）。

### 方式 B：跑构建好的镜像（不经 K8s，可选）

不搭 K8s、直接容器化运行镜像（前提：迁移已按方式 A 应用过——镜像只跑服务，不跑迁移）。

```bash
# Apple container（容器同在 VM 默认网络，直连 pg-dev 的 VM IP）
container build -t axum-k8s-app:local .
container ls --all | grep "pg-dev" # 查 pg-dev 的 VM IP
container run -d --name app-dev \
  -e DATABASE_URL='postgresql://app:dev-password@<pg-dev 的 VM IP>:5432/app' \
  -e RUST_LOG=info \
  axum-k8s-app:local

# Docker 等价命令（建自定义网络，容器名互解析；端口映射在 Docker Desktop 下正常）
docker build -t axum-k8s-app:local .
docker network create app-dev-net                     # 已有 pg-dev 时：docker network connect app-dev-net pg-dev
docker run -d --name app-dev --network app-dev-net \
  -e DATABASE_URL='postgresql://app:dev-password@pg-dev:5432/app' \
  -e RUST_LOG=info -p 8080:8080 \
  axum-k8s-app:local
```

验证与管理：

```bash
curl -s http://localhost:8080/healthz                 # 端口转发正常时；本机异常则直连 app-dev 的 VM IP
container logs app-dev                                # 看日志（JSON 行）；Docker 用 docker logs
container stop app-dev && container start app-dev     # 停止/再启动（优雅关闭）
container delete app-dev                              # 删除容器（镜像还在，随时可再 run）
```

---

## 二、构建镜像

```bash
# 两种运行时命令等价
container build -t <registry>/axum-k8s-app:<tag> .    # Apple container
docker build -t <registry>/axum-k8s-app:<tag> .       # Docker
# 推送：container image push / docker push（本地集群可免推送，见「三、K8s 部署」load-image）
```

镜像内容：

| 内容 | 用途 |
| ---- | ---- |
| `/usr/local/bin/app` | 服务二进制（ENTRYPOINT） |
| `/usr/local/bin/cli` | 迁移 CLI（K8s initContainer 执行 `cli migration apply`） |
| `/app/Toasty.toml` + `/app/toasty/` | 迁移配置与 SQL 文件（`Config::load()` 从工作目录读取） |

构建说明：多阶段构建（builder `rust:1.97-alpine` → runtime `alpine:3.24`，musl 静态二进制，非 root 数字 UID 1001）；先占位 main 构建缓存依赖层；`--locked` 要求 Cargo.lock 与 Cargo.toml 匹配（两者都提交 git）。

---

## 三、K8s 部署

### 3.1 本地测试集群（无现成集群时）

Apple container 1.2.1+ 内置 k8s 插件（推荐，kind 风格单节点），Docker 用户用 kind：

```bash
# Apple container：创建集群（自动写 kubeconfig 并设为当前 context）
container k8s create --name axum-dev

# 坑 1（仅老版本）：本机宿主端口转发在早期 Apple container 下失效（127.0.0.1:6445 TLS 握手被重置）
# 1.2.2 实测默认配置直接可用，无需改；老版本执行：
# kubectl config set-cluster axum-dev --server=https://<节点IP>:6443   # 节点 IP 查 `container k8s list` 的 ADDR 列

# 坑 2：load-image 用完整名 docker.io/library/<repo>:<tag>，
#    deployment.yaml 保持短名即可——kubelet 会自动规范化为 docker.io/library/ 前缀去 containerd 查找。
#    若加载用短名 / 镜像用长名，两边错位会 ImagePullBackOff
container image tag axum-k8s-app:0.1.0 docker.io/library/axum-k8s-app:0.1.0
container k8s load-image --name axum-dev docker.io/library/axum-k8s-app:0.1.0

# 集群管理：container k8s list / start / delete（删集群会清理 kubeconfig 条目）
```

### 3.2 部署步骤（速查）

按顺序执行：

```bash
# 1. 命名空间 + Secret（连接串不进 git）+ ConfigMap
kubectl apply -f k8s/namespace.yaml
kubectl create secret generic app-secret -n app \
  --from-literal=DATABASE_URL='postgresql://user:password@pg-host:5432/app' \
  --dry-run=client -o yaml | kubectl apply -f -
kubectl apply -f k8s/configmap.yaml

# 2. 应用清单（deployment.yaml 中两处 image 替换为实际镜像；initContainer 自动跑迁移）
kubectl apply -f k8s/deployment.yaml
kubectl apply -f k8s/service.yaml
kubectl apply -f k8s/ingress.yaml        # 域名 api.example.com；集群未装 ingress-nginx 时跳过，本地用 port-forward
kubectl apply -f k8s/fluent-bit.yaml     # 日志采集（monitoring 命名空间）

# 3. 验证（等待 3 副本就绪）
kubectl -n app rollout status deploy/app
kubectl -n app port-forward svc/app 8080:8080 &
curl -s http://localhost:8080/api/users
```

可选组件（日志链路）：`helm install loki grafana/loki --namespace monitoring --create-namespace` + `helm install grafana grafana/grafana --namespace monitoring`（fluent-bit 由 k8s/fluent-bit.yaml 提供，勿再装 loki-stack 避免重复采集；Grafana 数据源填 `http://loki.monitoring:3100`）。

### 3.3 升级与回滚

```bash
# 升级：deployment.yaml 里 initContainer(migrate) 和主容器(app) 共用同一镜像，
#       升级必须同步改两个，否则 migrate 仍跑旧 cli，新迁移可能执行不完整
kubectl -n app set image deploy/app \
  app=<registry>/axum-k8s-app:<new-tag> \
  migrate=<registry>/axum-k8s-app:<new-tag>
kubectl -n app rollout status deploy/app
# 回滚
kubectl -n app rollout undo deploy/app [--to-revision=<rev>]
```

要点：

- **镜像 tag 必须不可变**：一次构建对应一个 tag，**绝不能用同一 tag 反复覆盖**。否则 `rollout undo` 拉回的还是被覆盖过的最新版，回滚失效。staging 用 `:staging` 覆盖虽然回滚不能用，但能接受；prod 必须 `:<git-sha>`（或 release 号），CI 每次构建 push 全新 tag
- **迁移只前进**：`rollout undo` 只换回旧代码，数据库停留在最新 schema。迁移只做向后兼容变更（加列/加表永远安全；删列/改类型/重命名是危险操作，需 expand-contract 两阶段：加新列 → 代码切换 → 下个版本再删旧列），破坏性操作延后两个版本发布
- 回滚前自查：自上次 apply 以来有没有删列/删表/改类型/重命名？有且旧代码还在读取 → 不能直接回滚

---

## 四、环境区分（本地/测试/生产）

应用配置全部来自环境变量，**代码里没有任何环境判断**——同一份镜像可部署到任何环境，差异只在注入的配置与 K8s 编排（ConfigMap/Secret、namespace、副本数、域名、tag）。

| 维度 | 本地 local | 测试 staging | 生产 prod |
| ---- | ---- | ---- | ---- |
| 运行方式 | cargo 直跑或本地容器 | K8s，namespace `app-staging` | K8s，namespace `app` |
| 数据库 | 本地容器 PG18（pg-dev） | 独立 PG 库 | 托管/高可用 PG |
| 访问入口 | `localhost:8080`（cargo 直跑）/ `<容器 VM IP>:8080`（本地容器，参考六、故障排查） | `staging-api.example.com` | `api.example.com` |
| 镜像 tag | — | `:staging`（可覆盖，**牺牲回滚能力**） | `:<release-sha>`（不可变） |
| 副本数 | 1 进程 | 2 | 3 |

镜像 tag 策略：staging 用 `:staging`（每次构建覆盖、无需追溯，回滚需手动指定老版本）；prod 用 `:<git-sha>`（不可变、回滚依赖）。

多环境清单推荐 kustomize overlays（`k8s/base/` + `k8s/overlays/{staging,prod}/`，`kubectl apply -k`）：base 放公共清单，overlay 覆写 namespace/副本数/域名/tag。各环境独立跑迁移（`__toasty_migrations` 表各自记录，幂等），日志按 namespace label 天然分库。

---

## 五、接口与配置

接口：

| 方法 | 路径 | 说明 | 状态码 |
| ---- | ---- | ---- | ---- |
| GET | `/healthz` | 存活探针 | 200 |
| GET | `/readyz` | 就绪探针（DB 探活） | 200 / 503 |
| GET/POST | `/api/users` | 列表 / 创建 `{"name","email"}` | 200 / 400 / 409 |
| GET/PUT/DELETE | `/api/users/{id}` | 详情 / 更新 / 删除 | 200 / 404 等 |
| GET | `/unusual` | **模拟崩溃**（`std::process::abort()`，进程被 SIGABRT） | 无响应 |

错误统一格式：`{"error":{"code":"not_found|bad_request|conflict|internal","message":"..."}}`

环境变量：

| 变量 | 必填 | 默认 | 说明 |
| ---- | ---- | ---- | ---- |
| `DATABASE_URL` | ✅ | — | PostgreSQL 连接串 |
| `BIND_ADDR` | | `0.0.0.0:8080` | HTTP 监听地址 |
| `RUST_LOG` | | `info` | tracing 级别（error/warn/info/debug/trace） |
| `DB_POOL_MAX_SIZE` | | `10` | 连接池上限（副本数 × 池大小 ≤ PG `max_connections`） |

迁移 CLI：`cargo run --bin cli -- migration generate --name <name>`（模型变更 → 增量 SQL）、`migration apply`（幂等）、`migration snapshot`（打印 schema）、`migration drop / reset`（仅开发环境，生产禁用——`drop` 删除某次迁移的历史记录（不影响数据库），`reset` 撤销所有迁移并删表）。

---

## 六、故障排查

| 症状 | 排查 |
| ---- | ---- |
| 本地连 PG 报 `No route to host (os error 65)` / `Connection reset by peer`，或 curl 方式 B 的 `app-dev` 容器端口无响应 | **Apple container 宿主端口转发异常**（本机实测 5432/8080：TCP 可连但数据交换被 reset，或直接 no route to host）。方式 A 的 `cargo run` 直接 listen 在宿主机 8080，不经端口转发不受影响。`container inspect <容器名>` 查 `ipv4Address`（VM 网段 192.168.64.x），客户端/连接串直连该 IP |
| Pod 卡在 `Init:CrashLoopBackOff` | `kubectl -n app logs deploy/app -c migrate`——迁移 SQL 报错 |
| Pod `CrashLoopBackOff` | `kubectl -n app logs deploy/app`——通常是 DATABASE_URL 不可达（错填、跨网络不通、PG 容器未起） |
| readiness 失败（Pod Running 但不 Ready） | DB 连接问题或 PG `max_connections` 打满（调 `DB_POOL_MAX_SIZE`） |
| 接口 503/超时 | `kubectl -n app get endpointslices -l kubernetes.io/service-name=app`（v1.33+ 弃用 endpoints）检查 Service 端点 |
| 创建用户 409 | 预期行为：email 已存在（应用层预检 → DB 唯一约束兜底并发竞态） |
| 日志未进 Loki | `kubectl -n monitoring logs ds/fluent-bit`；确认 Loki service 名与 fluent-bit.conf 一致 |
| Grafana 查不到日志 | datasource URL 填 `http://loki.monitoring:3100`；先用 `{namespace="app"}` 简化查询确认数据存在 |

---

## 设计要点

- **迁移与运行时模型一致**：`bin/cli.rs` 与 server 共用 `src/lib.rs` 的 `build_db()` 与模型定义（`toasty::models!(crate::*)`）
- **探针与日志分离**：探针路由不经过请求日志中间件，避免 K8s 3s 一次的探测刷屏
- **优雅关闭双保险**：K8s `preStop sleep 10`（Ingress 摘流）+ 应用内 `with_graceful_shutdown` 排空在途请求
- **应用不持久化日志**：JSON 行写 stdout，Fluent Bit 采集后送 Loki（Grafana 查询）
- **`/unusual` 端点专门用于 K8s 自愈测试**：`std::process::abort()` 触发 SIGABRT 让整个进程退出（handler 里 `panic!` 只会杀死当前 tokio 任务，进程照常存活、liveness 探针照常通过——达不到测试目的）。调用后 K8s 会重启 Pod，重启时 initContainer 也会重新跑一遍迁移（幂等）
