# Kubernetes 部署手册

日志链路：`应用 tracing JSON → stdout → Fluent Bit(DaemonSet) → Loki → Grafana 查询`。

## 0. 前置条件

- Kubernetes 集群 + `kubectl`（本地测试：Apple container 1.2.1+ 用内置 k8s 插件，Docker 用户用 kind/minikube/k3s——见根 README「三、K8s 部署」3.1 节）
- 已安装 **ingress-nginx**：`kubectl get ingressclass nginx`
- 已安装 **Helm**
- 可访问的 PostgreSQL（外部/托管，或集群内自建）

## 1. 准备镜像

构建并推送命令见根 README「二、构建镜像」（`container build`/`docker build` 等价）。推送后**将 k8s/deployment.yaml 中两处 image（initContainer + 主容器）替换为 `<registry>/axum-k8s-app:<tag>`**。

本地集群免 registry——构建后直接 load，**此时不要替换 deployment.yaml 的 image**：保持短名 `axum-k8s-app:<tag>`，kubelet 会规范化为 `docker.io/library/` 前缀，正好命中下面加载的完整名镜像：

```bash
# ⚠️ load-image 必须用完整名 docker.io/library/<repo>:<tag>，deployment.yaml 保持短名即可。
#    短名加载也能成功，但 kubelet 拉取时会规范化为 docker.io/library/ 前缀去 containerd 查找；
#    加载用短名 + 拉取自动补前缀 → 两边错位 → ImagePullBackOff
#    （完整名加载 + 短名部署：kubelet 补前缀后正好命中加载的镜像 ✅）
container image tag axum-k8s-app:<tag> docker.io/library/axum-k8s-app:<tag>
container k8s load-image --name <cluster> docker.io/library/axum-k8s-app:<tag>
# Docker（kind）
kind load docker-image axum-k8s-app:<tag>
```

镜像内含：server 二进制、迁移 CLI 二进制、`Toasty.toml` 与 `toasty/` 迁移文件（initContainer 使用）。

## 2. 安装 Loki + Grafana（Helm）

```bash
helm repo add grafana https://grafana.github.io/helm-charts
helm repo update

# grafana/loki-stack 已弃用（helm 会报 deprecated），分两个 chart 安装
helm install loki grafana/loki \
  --namespace monitoring --create-namespace
helm install grafana grafana/grafana \
  --namespace monitoring \
  --set persistence.enabled=true
```

> 本仓库自带 Fluent Bit DaemonSet（k8s/fluent-bit.yaml），勿再装 loki-stack 内嵌采集器，避免重复采集。默认 `loki` release 名 → Loki 服务 `loki.monitoring:3100`，与 fluent-bit.yaml 的 output 一致。

## 3. 部署应用

```bash
kubectl apply -f k8s/namespace.yaml

# Secret：k8s/secret.yaml 是占位模板，勿直接 apply，用下面命令创建
kubectl create secret generic app-secret -n app \
  --from-literal=DATABASE_URL='postgresql://user:password@pg-host:5432/app' \
  --dry-run=client -o yaml | kubectl apply -f -

kubectl apply -f k8s/configmap.yaml
kubectl apply -f k8s/deployment.yaml   # initContainer 自动跑 migration apply
kubectl apply -f k8s/service.yaml
kubectl apply -f k8s/ingress.yaml      # 域名 api.example.com；集群未装 ingress-nginx 时跳过，本地用 port-forward 验证
kubectl apply -f k8s/fluent-bit.yaml   # 日志采集（monitoring 命名空间）
```

迁移：新版本由 initContainer 自动 apply 增量迁移，无需手动执行（生成新迁移见根 README「五」迁移 CLI；迁移只前进、回滚注意见根 README 3.3）。

## 4. 验证与更新

```bash
kubectl -n app rollout status deploy/app   # 等 3 副本就绪

# 探针（镜像不含 curl，用 port-forward 从本机探测）
kubectl -n app port-forward svc/app 8080:8080 &
curl -s http://localhost:8080/healthz
curl -s http://localhost:8080/readyz
curl -s http://localhost:8080/api/users     # 业务接口（本地用 port-forward 即可，无需域名）

# 模拟滚动发布，观察优雅关闭
kubectl -n app rollout restart deploy/app
kubectl -n app logs deploy/app --tail=20 -f   # 应看到 shutdown signal received → server shut down gracefully

# 配置变更（如 RUST_LOG=debug）：改 k8s/configmap.yaml 后重新 apply，再 rollout restart 生效
```

## 5. 日志查询（Grafana + Loki）

```bash
kubectl -n monitoring port-forward svc/grafana 3000:80
# 默认账号 admin / admin（首次登录修改）
```

Grafana 中 **Add data source → Loki**，URL 填 `http://loki.monitoring:3100`。查询示例（应用日志结构化字段可直接过滤）：

```logql
{job="fluentbit", namespace="app"} | json | status > 500
{job="fluentbit", namespace="app"} | json | level = "error"
```

## 6. 故障排查

| 症状              | 排查                                                                           |
| ----------------- | ------------------------------------------------------------------------------ |
| Pod 卡在 Init     | `kubectl -n app logs deploy/app -c migrate`——迁移 SQL 报错                     |
| readiness 失败    | `kubectl -n app logs deploy/app`；检查 DATABASE_URL、PG `max_connections`      |
| Fluent Bit 不工作 | `kubectl -n monitoring logs ds/fluent-bit`；确认 loki service 名与 output 一致 |
| 域名不通          | `kubectl -n app get ingress`；确认 DNS 记录指向 ingress-nginx LoadBalancer     |

（数据库连接、回滚等其他问题见根 README「六、故障排查」。）
