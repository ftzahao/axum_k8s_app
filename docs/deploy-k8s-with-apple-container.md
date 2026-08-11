# Apple container + 本地 K8s 部署实战教程

> 适用 `axum_k8s_app` 项目；本机实测环境：macOS（Darwin arm64）、Apple `container` CLI 1.2.2、`kubectl` v1.36.3、`helm` v4.2.3。
> 教程内容全部来自本机实际跑通的过程，不是「按文档应该这样」的复述。

---

## 0. 目标 & 拓扑

把一个 Rust（Axum 0.8 + Toasty ORM + PostgreSQL）应用从源码到 K8s 集群跑通业务接口。

本地网络拓扑（Apple container 的两个 VM 都跑在 192.168.64.0/24 网段）：

| 角色           | 所在 VM     | IP               | 说明                                                            |
| -------------- | ----------- | ---------------- | --------------------------------------------------------------- |
| 宿主机         | macOS       | 192.168.3.x      | Apple container 的 host                                         |
| PostgreSQL     | 容器 VM     | **192.168.64.2** | 用 `container run` 起的 PG18                                    |
| K8s 节点       | 容器 VM     | **192.168.64.3** | `container k8s create` 起的单节点集群（同时也是 control-plane） |
| kube-apiserver | 同 K8s 节点 | 6443 端口        | 宿主通过 127.0.0.1:6445 端口转发访问（实测可用，无需改节点 IP） |

两个 VM 之间的网络是互通的，K8s 节点能直接 `nc 192.168.64.2 5432` 命中 PG。

---

## 1. 前置准备

```bash
# 1.1 启动 Apple container 后台服务（k8s 插件需要它先起来）
container system start
# 看到 "Verifying machine API server is running..." 即就绪

# 1.2 确认工具链
container --version    # container CLI version 1.2.2
kubectl version --client=true
helm version
```

---

## 2. 起 PostgreSQL（dev 库）

```bash
container run -d --name pg-dev \
  -e POSTGRES_USER=app -e POSTGRES_PASSWORD=dev-password -e POSTGRES_DB=app \
  -p 5432:5432 docker.io/library/postgres:18
# 输出：pg-dev
```

等就绪 + 拿 VM IP：

```bash
container logs pg-dev 2>&1 | grep "ready to accept connections"
# 2026-08-11 03:14:52.172 UTC [47] LOG:  database system is ready to accept connections

container inspect pg-dev | awk '/ipv4Address/ {print $2}' | tr -d ',"' | head -1
# 192.168.64.2
```

> ⚠️ **踩坑记录**：README 提到「Apple container 宿主端口转发异常，TCP 能连但数据交换被 reset」。本机实测 5432 端口转发在某些进程下表现不稳定（首次 cargo 连报 `No route to host`，重试就 OK）。最稳的做法：客户端一律走 VM IP `192.168.64.2`，**不要用 `localhost`**。这一点也会影响 K8s 里 Pod 的连接串。

---

## 3. 应用数据库迁移（开发期一次性）

迁移用项目内置的 CLI（`bin/cli.rs`），跟 server 共用 `build_db()` 和模型定义。

```bash
export DATABASE_URL='postgresql://app:dev-password@192.168.64.2:5432/app'
cargo run --bin cli -- migration apply
```

预期输出（成功）：

```
  Apply Migrations

  Connected to postgresql://app:***@192.168.64.2:5432/app

  → Found 1 pending migration(s) to apply
  → Applying migration: 0000_initial.sql
  ✓ Applied: 0000_initial.sql

  Successfully applied 1 migration(s)
```

> 💡 本步在「开发期」跑一次就好。生产/集群里不需要再跑——K8s 里的 `initContainer` 会在每个新 Pod 启动时自动 apply 迁移，且 toasty 迁移是**幂等的**（通过 `__toasty_migrations` 表记录已应用版本），重跑无副作用。本机本次实测：Pod 启动时 initContainer 输出 `All migrations are already applied. Database is up to date.`，验证了幂等性。

---

## 4. 创建 K8s 集群

```bash
container k8s create --name axum-dev
# 等待约 1 分钟，会自动写 kubeconfig 并设为当前 context
```

验证：

```bash
container k8s list
# CLUSTER   NODE      ROLE           STATE    CPUS  MEMORY    ADDR          PORTS
# axum-dev  axum-dev  control-plane  running  3     12288 MB  192.168.64.3  6445->6443

kubectl config get-contexts
# CURRENT   NAME       CLUSTER    AUTHINFO   NAMESPACE
# *         axum-dev   axum-dev   axum-dev

kubectl get nodes
# NAME       STATUS   ROLES           AGE     VERSION
# axum-dev   Ready    control-plane   2m13s   v1.35.5
```

> ⚠️ **版本注意**：根 README 里"坑 1"描述的是早期 Apple container 的 `127.0.0.1:6445` TLS 握手被重置问题。本机实测 **1.2.2 版本**默认配置可直接用，无需修改。如果你的版本更老或表现不稳，再按 `kubectl config set-cluster axum-dev --server=https://192.168.64.3:6443` 方式绕过。

---

## 5. 构建应用镜像

多阶段构建（builder: `rust:1.97-alpine` → runtime: `alpine:3.24`，musl 静态二进制 + 非 root）。

```bash
container build -t axum-k8s-app:0.1.0 .
```

输出关键行（实测）：

```
#15 [linux/arm64 builder 6/7] RUN cargo build --release --locked
#15 14.43     Finished `release` profile [optimized] target(s) in 14.39s
#20 exporting to oci image format
#20 exporting manifest list sha256:5030ca7eab1951c36658f979d85b0652f33b9e9ee323df61523748f30798a336
axum-k8s-app:0.1.0
```

> 第二次构建会复用 builder 依赖层缓存，秒级完成。

---

## 6. 把镜像加载到 K8s 集群

> ⚠️ **踩坑记录**：load-image 必须用完整名 `docker.io/library/<repo>:<tag>`，deployment.yaml 保持短名即可。短名加载本身也能成功，但 kubelet 拉取时会规范化为 `docker.io/library/...` 前缀去 containerd 查找——加载用短名 + 拉取自动补前缀，两边错位 → `ImagePullBackOff`。

```bash
# 6.1 打完整名 tag
container image tag axum-k8s-app:0.1.0 docker.io/library/axum-k8s-app:0.1.0

# 6.2 加载到集群
container k8s load-image --name axum-dev docker.io/library/axum-k8s-app:0.1.0
# Saving image: ["ref": docker.io/library/axum-k8s-app:0.1.0]
# Importing image into cluster: ["target": axum-dev]
```

**关键**：`k8s/deployment.yaml` 里**保持短名 `axum-k8s-app:0.1.0`** 不要改——kubelet 拉取时会自动规范化为 `docker.io/library/axum-k8s-app:0.1.0`，正好命中刚加载的完整名镜像。`imagePullPolicy: IfNotPresent` 保证已加载的镜像不再走网络拉取。

---

## 7. 部署应用到 K8s

按顺序 apply 清单 + 创建 Secret：

```bash
# 7.1 namespace
kubectl apply -f k8s/namespace.yaml
# namespace/app created

# 7.2 Secret（敏感信息不进 git，用 --from-literal 现做）
kubectl create secret generic app-secret -n app \
  --from-literal=DATABASE_URL='postgresql://app:dev-password@192.168.64.2:5432/app' \
  --dry-run=client -o yaml | kubectl apply -f -
# secret/app-secret created

# 7.3 ConfigMap
kubectl apply -f k8s/configmap.yaml
# configmap/app-config created

# 7.4 Deployment（initContainer 自动跑 migration）
kubectl apply -f k8s/deployment.yaml
# deployment.apps/app created

# 7.5 Service
kubectl apply -f k8s/service.yaml
# service/app created
```

> 🔑 **本步关键决策**：Secret 里的 `DATABASE_URL` 必须是 **K8s 节点能直连的 PG 地址**。K8s 节点在 192.168.64.3，PG 在 192.168.64.2，所以填 `192.168.64.2`。如果 PG 跑在集群外的云上，填云端内网域名或 IP。
>
> 另一个常见选项：把 PG 本身也用 StatefulSet 跑在集群内，连接串写成 `pg-cluster.default.svc.cluster.local`。本次教程为简化，PG 跑在容器 VM 上。

---

## 8. 验证部署

### 8.1 看 Pod 状态

```bash
kubectl -n app get pods
# NAME                   READY   STATUS    RESTARTS   AGE
# app-79d578c49b-7hb2x   1/1     Running   0          23s
# app-79d578c49b-r92xr   1/1     Running   0          23s
# app-79d578c49b-v229c   1/1     Running   0          23s
```

3 副本全部 Running，**30 秒内就绪**（包含 initContainer 跑迁移的时间）。

### 8.2 看 initContainer 迁移日志

```bash
kubectl -n app logs deploy/app -c migrate
#   Apply Migrations
#   Connected to postgresql://app:***@192.168.64.2:5432/app
#   All migrations are already applied. Database is up to date.
```

幂等命中，符合预期。

### 8.3 看主容器日志

```bash
kubectl -n app logs deploy/app --tail=10
```

预期看到 JSON 行：

```json
{"timestamp":"...","level":"INFO","fields":{"message":"schema built successfully","tables":1}}
{"timestamp":"...","level":"INFO","fields":{"message":"database ready"}}
{"timestamp":"...","level":"INFO","fields":{"message":"database connection pool ready"}}
{"timestamp":"...","level":"INFO","fields":{"message":"listening","addr":"0.0.0.0:8080"}}
{"timestamp":"...","level":"INFO","fields":{"message":"request completed","status":200,"latency_ms":7},"spans":[{"method":"GET","path":"/readyz",...}]}
```

### 8.4 看 Service 端点

```bash
kubectl -n app get endpoints app
# NAME   ENDPOINTS                                         AGE
# app    10.244.0.6:8080,10.244.0.7:8080,10.244.0.8:8080   35s
```

3 个 Pod 都在 Endpoint 中，Service 正常分发。

### 8.5 调业务接口（port-forward）

```bash
kubectl -n app port-forward svc/app 8080:8080 &
# 探针
curl -s http://localhost:8080/healthz
# {"status":"ok"}
curl -s http://localhost:8080/readyz
# {"db":"up","status":"ok"}
# 业务
curl -s -X POST http://localhost:8080/api/users \
  -H 'Content-Type: application/json' \
  -d '{"name":"Alice","email":"alice@example.com"}'
# {"id":1,"name":"Alice","email":"alice@example.com","created_at":"..."}
curl -s http://localhost:8080/api/users
# [{"id":1,"name":"Alice",...},{"id":2,"name":"Bob",...}]

# 其他接口（PUT/DELETE/GET 详情）类似，可自行测试；唯一约束 email 重复会返回 409
# 自愈测试（让 Pod 重启，观察 initContainer 重新跑迁移）：
curl -s http://localhost:8080/unusual
# 此时 Pod 进程 SIGABRT 退出 → K8s 重建 Pod → 新 Pod 启动时 initContainer 跑迁移（幂等命中）

# 收尾
pkill -f "port-forward svc/app"
```

---

## 9. 实测踩坑清单

| #   | 现象                                                                    | 根因                                               | 解决                                                                                                     |
| --- | ----------------------------------------------------------------------- | -------------------------------------------------- | -------------------------------------------------------------------------------------------------------- |
| 1   | `cargo run ... migration apply` 首次报 `No route to host (os error 65)` | Apple container VM 网段路由在某些进程下偶发不通    | 重试即可；若仍不通，确认 `container inspect pg-dev` 的 `ipv4Address` 仍有效                              |
| 2   | `kubectl get nodes` 默认配置不能用（README 提到）                       | 127.0.0.1:6445 端口转发在旧版 Apple container 失效 | 1.2.2 已修复，无需操作；老版本：`kubectl config set-cluster axum-dev --server=https://192.168.64.3:6443` |
| 3   | Pod `ImagePullBackOff`                                                  | 镜像用短名加载，kubelet 找不到                     | `container image tag` 打完整名 `docker.io/library/...` 再 `load-image`；deployment.yaml 保持短名         |
| 4   | Pod `CrashLoopBackOff` + 日志 `connection refused`                      | Secret 里 `DATABASE_URL` 用了 `localhost`          | 改用 PG 容器 VM IP（本次 `192.168.64.2`），或把 PG 跑在集群内用 Service DNS                              |
| 5   | `kubectl run ... -it --rm` 报 `executable file not found`               | `--rm` 跟 `--command` 冲突                         | 用 `--command -- sh -c "..."` 不带 `-it --rm`；或先 run 再 attach                                        |

---

## 10. 常用管理命令

```bash
# 集群
container k8s list                                    # 列出集群
container k8s start --name axum-dev                  # 启动已停止的集群
container k8s delete --name axum-dev                 # 删除集群（同时清理 kubeconfig）

# 应用
kubectl -n app get pods,svc,cm,secret
kubectl -n app rollout status deploy/app             # 等滚动完成
kubectl -n app rollout restart deploy/app            # 重启（触发 initContainer 跑迁移）
kubectl -n app rollout undo deploy/app               # 回滚
kubectl -n app set image deploy/app \
  app=docker.io/library/axum-k8s-app:0.2.0 \
  migrate=docker.io/library/axum-k8s-app:0.2.0       # 升级镜像（migrate 容器必须同步）

# 日志
kubectl -n app logs deploy/app --tail=20 -f          # 实时日志
kubectl -n app logs deploy/app -c migrate --previous  # 上次启动的迁移日志

# 调试
kubectl -n app exec -it <pod> -- sh                   # 进容器（alpine 镜像有 sh）
kubectl -n app port-forward svc/app 8080:8080        # 端口转发
kubectl -n app describe pod <pod>                    # 看事件
```

---

## 11. 跟根 README 修订后版本的差异记录

| 项                           | 原 README                                   | 本次实测 → 根 README 修订                                                                                  |
| ---------------------------- | ------------------------------------------- | ---------------------------------------------------------------------------------------------------------- |
| kubectl `server` 地址        | 提示要改成节点 IP                           | 1.2.2 默认配置直接可用 → 根 README 坑 1 改为「仅老版本」                                                   |
| 宿主端口转发 (`-p`) 错误信息 | 写「Connection reset by peer」              | 实测还可能是 `No route to host` → 根 README 故障表已补充                                                   |
| K8s 节点访问 PG              | 未涉及                                      | 必须用 PG 的 VM IP，**不能**用 `localhost` 或 `pg-dev`（容器名只在容器 VM 内解析）→ 已写入根 README 故障表 |
| 升级命令                     | 只 `set image deploy/app app=...`           | 漏改 `migrate` 容器，新版迁移可能执行不完整 → 根 README 升级示例已改为同时改两个容器                       |
| 短名 vs 完整名镜像加载       | 「短名加载会 ImagePullBackOff」（措辞不准） | 短名加载也能成功，是 Pod 拉取时找不到 → k8s/README 注释已重写说明「加载/拉取两边错位」                     |

---

## 12. 配套：清理（结束本教程时跑）

```bash
# 应用（namespace 级资源一并删除 namespace 时会级联清理，这里单独删便于观察）
kubectl delete -f k8s/service.yaml
kubectl delete -f k8s/deployment.yaml
kubectl delete -f k8s/configmap.yaml
kubectl delete -n app secret app-secret
kubectl delete -f k8s/namespace.yaml

# 集群
container k8s delete --name axum-dev

# PG
container stop pg-dev && container delete pg-dev

# 镜像
container image rm axum-k8s-app:0.1.0 docker.io/library/axum-k8s-app:0.1.0

# （如果按 k8s/README 装了 Loki + Grafana + fluent-bit：kubectl delete -f k8s/fluent-bit.yaml；
#  helm uninstall loki grafana -n monitoring；kubectl delete namespace monitoring）
```

---

## 参考

- 项目根 `README.md`：应用架构、迁移策略、回滚注意事项、生产/测试环境区分
- `k8s/README.md`：K8s 部署清单逐项说明 + Loki/Grafana 日志链路（本次教程未涉及日志栈，按需启用）
- Apple container k8s 插件：`container k8s --help`
