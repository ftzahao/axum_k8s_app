# 本地 / 测试 / 生产环境区分说明

本文档说明本项目如何区分本地（local）、测试（staging）、生产（prod）三个环境，以及推荐的落地方式（kustomize overlays）。

## 1. 设计原则：环境差异在配置与编排层，不在代码

应用配置全部来自环境变量（[src/config.rs](src/config.rs)），代码中**没有任何环境判断逻辑**。这是刻意设计：

- **同一份镜像可部署到任何环境**——镜像与环境无关
- 环境差异全部收敛在两处：
  1. **注入什么环境变量**（ConfigMap / Secret / shell）
  2. **K8s 编排差异**（namespace、域名、副本数、资源限制、镜像 tag）
- 不需要 `APP_ENV` 分支逻辑，避免"测试代码带进生产"（例如本地调试开关被一并上线）

对比：在代码里写 `if env == "prod" { ... }` 是反面模式——分支会随环境数增长，且每一条分支都是潜在的测试/生产行为漂移点。

## 2. 三环境概览

| 维度 | 本地 local | 测试 staging | 生产 prod |
|---|---|---|---|
| 运行方式 | `cargo run` 直跑进程 | K8s，namespace `app-staging` | K8s，namespace `app` |
| 数据库 | 本地容器 PG18（`pg-dev`） | 独立 PG 库（可重置） | 托管/高可用 PG |
| 访问入口 | `localhost:8080` | `staging-api.example.com` | `api.example.com` |
| `RUST_LOG` | `debug` | `info`（排障临时切 debug） | `info` |
| 镜像 tag | —（源码直跑，不经镜像） | `:staging` 或 `:<sha>` | `:<release-sha>`（不可变） |
| 副本数 | 1 进程 | 2 | 3 |
| 资源限制 | — | 小规格 | 生产规格（见 deployment.yaml） |
| 探针 | 手动 curl | liveness + readiness | 同左 |
| Secret | shell 环境变量 | 测试值（明文可接受） | Vault / SealedSecret |
| 数据 | 随意重建 | 伪数据 | 真实数据 + 备份 |

## 3. 环境隔离的天然边界（零额外代码）

| 关注点 | 隔离机制 |
|---|---|
| **迁移** | initContainer 读各环境的 `DATABASE_URL` → 每个环境只在**自己的库**上跑 `migration apply`；apply 幂等（`__toasty_migrations` 表记录），重复发布安全 |
| **日志** | Fluent Bit 附加的 `namespace` label 即环境标识 → Loki 里 `{namespace="app"}` 与 `{namespace="app-staging"}` 天然分库，互不混淆 |
| **配置** | 每个环境一套 ConfigMap + Secret，注入方式相同、取值不同 |
| **数据** | 各环境独立数据库连接串，测试库永远不会连到生产库（除非 Secret 配错——见第 7 节检查项） |

## 4. 本地环境（Local）

不涉及 K8s，进程直跑：

```bash
# 1. 本地 PG（见 INSTALL.md 3.1；两种运行时任选其一）

# Apple container
container run -d --name pg-dev \
  -e POSTGRES_USER=app -e POSTGRES_PASSWORD=dev-password -e POSTGRES_DB=app \
  -p 5432:5432 docker.io/library/postgres:18

# Docker 等价命令
docker run -d --name pg-dev \
  -e POSTGRES_USER=app -e POSTGRES_PASSWORD=dev-password -e POSTGRES_DB=app \
  -p 5432:5432 postgres:18

# 2. 迁移 + 启动（环境变量来自 shell）
export DATABASE_URL='postgresql://app:dev-password@localhost:5432/app'
cargo run --bin cli -- migration apply
RUST_LOG=debug cargo run
```

本地环境特征：
- `RUST_LOG=debug` 看完整链路；`Ctrl-C` 走优雅关闭
- 数据可随意重建（删容器重来）；迁移文件 `toasty/` 是唯一需要提交进 git 的产物
- 可以并行跑多个实例区分端口，靠 JSON 日志的时间戳/span 辨别

## 5. 测试与生产：kustomize overlays（推荐方案）

### 5.1 目标目录结构

当前 `k8s/` 是单环境平铺清单（适合 INSTALL.md 的默认部署路径）。采用 kustomize 后演进为：

```
k8s/
├── namespace.yaml           # 基础设施：环境 Namespace（独立 apply，见 5.5）
├── fluent-bit.yaml          # 基础设施：日志采集 DaemonSet（全集群一份，独立 apply）
├── secret.yaml              # 模板占位（各环境实际用 kubectl create secret 注入）
├── base/                    # 应用清单（deployment/service/ingress/configmap 移入，
│   │                        #   namespace 字段由 overlay 统一覆写）
│   ├── kustomization.yaml
│   ├── deployment.yaml
│   ├── service.yaml
│   ├── ingress.yaml
│   └── configmap.yaml
└── overlays/
    ├── staging/             # 测试环境：namespace app-staging、副本 2、staging 域名
    │   ├── kustomization.yaml
    │   ├── patch-deployment.yaml
    │   ├── patch-configmap.yaml
    │   └── patch-ingress.yaml
    └── prod/                # 生产环境：namespace app、副本 3、正式域名
        ├── kustomization.yaml
        ├── patch-deployment.yaml
        └── patch-ingress.yaml
```

> **kustomize 是什么**：Kubernetes 内置的清单定制工具（`kubectl apply -k` 直接使用）。它把"同一份 base 清单 + 按环境打补丁"变成结构化目录，无需引入 Helm。当前 base 清单即 INSTALL.md 默认路径用的文件（namespace 覆写由 overlay 负责）。

### 5.2 base（公共部分）

`k8s/base/kustomization.yaml`：

```yaml
apiVersion: kustomize.config.k8s.io/v1beta1
kind: Kustomization
resources:
  - deployment.yaml
  - service.yaml
  - ingress.yaml
  - configmap.yaml
```

> deployment/service/ingress/configmap 四个文件内容与当前 `k8s/` 平铺版一致，唯一区别：文件里的 `namespace: app` 可保留（会被 overlay 的 `namespace:` 字段覆写）。**镜像 tag 不要写死在 base**——由 overlay 的 `images:` 指定（否则所有环境共用同一 tag，无法区分版本）。

### 5.3 overlays/staging（测试环境，完整文件）

`k8s/overlays/staging/kustomization.yaml`：

```yaml
apiVersion: kustomize.config.k8s.io/v1beta1
kind: Kustomization
namespace: app-staging          # ① 整套资源换 namespace
resources:
  - ../../base
images:
  - name: axum-k8s-app
    newName: <registry>/axum-k8s-app
    newTag: staging              # ② 测试环境固定 tag（见 5.6 tag 策略）
patches:
  - path: patch-deployment.yaml  # ③ 副本数/资源差异
  - path: patch-configmap.yaml   # ④ 配置取值差异
  - path: patch-ingress.yaml     # ⑤ 域名差异
```

`k8s/overlays/staging/patch-deployment.yaml`：

```yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: app
spec:
  replicas: 2
  template:
    spec:
      containers:
        - name: app
          resources:
            requests:
              cpu: 100m
              memory: 128Mi
            limits:
              cpu: 300m
              memory: 192Mi
```

`k8s/overlays/staging/patch-configmap.yaml`：

```yaml
apiVersion: v1
kind: ConfigMap
metadata:
  name: app-config
data:
  RUST_LOG: "info"
  DB_POOL_MAX_SIZE: "5"     # 2 副本 × 5 = 10 连接
```

`k8s/overlays/staging/patch-ingress.yaml`：

```yaml
apiVersion: networking.k8s.io/v1
kind: Ingress
metadata:
  name: app
spec:
  rules:
    - host: staging-api.example.com
      http:
        paths:
          - path: /
            pathType: Prefix
            backend:
              service:
                name: app
                port:
                  name: http
```

### 5.4 overlays/prod（生产环境）

与 staging 同构，差异点：

```yaml
# k8s/overlays/prod/kustomization.yaml（差异）
namespace: app                  # 生产 namespace
images:
  - name: axum-k8s-app
    newName: <registry>/axum-k8s-app
    newTag: <release-sha>       # 不可变 tag（git commit sha），保证可回滚
```

```yaml
# k8s/overlays/prod/patch-deployment.yaml
spec:
  replicas: 3                   # 与 base 一致时可不写，显式写清楚更保险
```

```yaml
# k8s/overlays/prod/patch-ingress.yaml
spec:
  rules:
    - host: api.example.com
      ...
```

### 5.5 基础设施与 Secret（不进 kustomize）

以下组件**每个集群只装一次**，独立于应用环境：

```bash
# Namespace（staging / prod 各一个）
kubectl create ns app-staging
kubectl create ns app

# 日志采集（全集群一份，采集所有 namespace）
kubectl apply -f k8s/fluent-bit.yaml

# Loki + Grafana（全集群一份，见 INSTALL.md 5.3）
helm install loki grafana/loki --namespace monitoring --create-namespace
helm install grafana grafana/grafana --namespace monitoring
```

Secret 按环境独立注入（值不进 git）：

```bash
kubectl -n app-staging create secret generic app-secret \
  --from-literal=DATABASE_URL='postgresql://app:test-password@pg-staging:5432/app' \
  --dry-run=client -o yaml | kubectl apply -f -

kubectl -n app create secret generic app-secret \
  --from-literal=DATABASE_URL='postgresql://app:real-password@pg-prod:5432/app' \
  --dry-run=client -o yaml | kubectl apply -f -
```

生产 Secret 建议升级为 SealedSecret / External Secrets Operator / Vault 管理（密钥不落人眼）。

### 5.6 镜像 tag 策略

| 环境 | tag | 说明 |
|---|---|---|
| staging | `:staging` | 每次构建覆盖；测试环境无需版本追溯 |
| prod | `:<git-sha>` | 不可变；`rollout undo` 依赖它，禁止用 `:latest`（回滚会拉回同一个 latest） |

两种运行时任选其一（Apple container 与 Docker 命令等价）：

```bash
# 构建 → 按环境打 tag → 推送

# Apple container
container build -t <registry>/axum-k8s-app:staging .
container build -t <registry>/axum-k8s-app:<git-sha> .
container image push <registry>/axum-k8s-app:staging <registry>/axum-k8s-app:<git-sha>

# Docker 等价命令
# docker build -t <registry>/axum-k8s-app:staging .
# docker build -t <registry>/axum-k8s-app:<git-sha> .
# docker push <registry>/axum-k8s-app:staging <registry>/axum-k8s-app:<git-sha>

# 发布到对应环境
kubectl apply -k k8s/overlays/staging   # 或仅更新镜像后：
kubectl -n app-staging set image deploy/app app=<registry>/axum-k8s-app:staging
```

### 5.7 部署命令一览

```bash
# 测试环境整套部署
kubectl apply -k k8s/overlays/staging

# 生产环境整套部署
kubectl apply -k k8s/overlays/prod

# 查看差异（apply 前审查改动）
kubectl diff -k k8s/overlays/prod
```

> 未采用 kustomize 时的单环境路径见 INSTALL.md 第 5 节；两套路径不冲突，二选一即可。

## 6. 应用层可选增强：日志环境标签

应用本身**不需要**知道环境。唯一可选的增强是在日志中标记环境，便于本地并行调试 / 后续接监控时按环境过滤：

```rust
// src/config.rs 增加可选项（默认空）
app_env: env::var("APP_ENV").unwrap_or_default(),

// src/main.rs 启动日志带上
tracing::info!(app_env = %config.app_env, "starting axum_k8s_app");
```

各环境 ConfigMap 注入 `APP_ENV: staging` / `APP_ENV: prod`。建议**暂不实现**——等真正需要（如报警分环境路由）再补，避免为区分而区分。

## 7. 常见问题

**Q: 为什么不在代码里写 `if env == "prod"`？**
A: 单镜像多环境是核心原则。分支逻辑每加一条都是行为漂移点，且难以测试覆盖；环境差异（域名、副本数、日志级别）本来就该由编排层表达。

**Q: 测试环境能连到生产库吗？**
A: 不会——除非 Secret 配错。部署后自查：

```bash
# 确认各环境 Pod 里的连接串指向各自数据库
kubectl -n app-staging exec deploy/app -- env | grep DATABASE_URL
kubectl -n app exec deploy/app -- env | grep DATABASE_URL
```

**Q: 测试环境的迁移和生产冲突吗？**
A: 不冲突。各环境在自己的库上 apply，`__toasty_migrations` 表各自记录；同一条迁移 SQL 可以在不同环境先后执行（幂等）。

**Q: 为什么生产禁止 `:latest`？**
A: `rollout undo` 回滚的是"镜像 tag 对应的历史版本"。`:latest` 每次构建都覆盖同一个 tag，回滚时拉回的仍是同一个 latest，回滚失效；且无法知道线上跑的是哪次构建。

**Q: staging 与 prod 的配置能共用吗？**
A: 资源、探针等结构共用（在 base）；取值分开（在 overlay）。`DB_POOL_MAX_SIZE` 这种与副本数耦合的取值（副本 × 池大小 ≤ PG `max_connections`）必须在 overlay 里按环境核算。
