# Kubernetes 部署手册

日志链路：`应用 tracing JSON → stdout → Fluent Bit(DaemonSet) → Loki → Grafana 查询`。

## 0. 前置条件

- Kubernetes 集群（如 kind/minikube/k3s）+ `kubectl`
- 已安装 **ingress-nginx**：`kubectl get ingressclass nginx`
- 已安装 **Helm**
- 可访问的 PostgreSQL（外部/托管，或集群内自建）

## 1. 构建并推送镜像

```bash
# 在项目根目录
docker build -t <registry>/axum-k8s-app:<tag> .
docker push <registry>/axum-k8s-app:<tag>
# 将 k8s/deployment.yaml 中两处 image 替换为 <registry>/axum-k8s-app:<tag>
```

镜像内含：server 二进制、迁移 CLI 二进制、`Toasty.toml` 与 `toasty/` 迁移文件（initContainer 使用）。

## 2. 安装 Loki + Grafana（Helm）

```bash
helm repo add grafana https://grafana.github.io/helm-charts
helm repo update

# Loki + Fluent Bit（可选）+ Grafana 一体
helm install loki grafana/loki-stack \
  --namespace monitoring --create-namespace \
  --set fluent-bit.enabled=false \
  --set grafana.enabled=true
```

> fluent-bit.enabled=false：本仓库自带更明确的 DaemonSet（k8s/fluent-bit.yaml），避免与 chart 内 Fluent Bit 冲突。
> 若使用默认 `loki` release 名，Loki 服务为 `loki.monitoring:3100`，与 fluent-bit.yaml 的 output 配置一致。

## 3. 部署应用

```bash
kubectl apply -f k8s/namespace.yaml

# 先准备 Secret：用真实连接串替换 base64 值
kubectl create secret generic app-secret -n app \
  --from-literal=DATABASE_URL='postgresql://user:password@pg-host:5432/app' \
  --dry-run=client -o yaml | kubectl apply -f -

kubectl apply -f k8s/configmap.yaml
kubectl apply -f k8s/deployment.yaml   # initContainer 会自动跑 migration apply
kubectl apply -f k8s/service.yaml
kubectl apply -f k8s/ingress.yaml
```

## 4. 验证

```bash
kubectl -n app get pods -w          # 等 3/3 Running（initContainer 先完成迁移）
kubectl -n app logs deploy/app      # 查看 JSON 请求日志

# 探针
kubectl -n app exec deploy/app -- curl -s http://localhost:8080/healthz
kubectl -n app exec deploy/app -- curl -s http://localhost:8080/readyz

# 业务接口（域名解析到集群后）
curl -s https://api.example.com/api/users

# 模拟滚动发布，观察优雅关闭
kubectl -n app rollout restart deploy/app
kubectl -n app logs -f deploy/app | tail -20   # 应看到 shutdown signal received → server shut down gracefully
```

## 5. 日志查询（Grafana + Loki）

```bash
kubectl -n monitoring get svc grafana
kubectl -n monitoring port-forward svc/grafana 3000:80
# 默认账号 admin / admin（首次登录修改）
```

- Grafana 中 **Add data source → Loki**，URL 填 `http://loki.monitoring:3100`
- 查询示例（应用日志结构化字段可直接过滤）：

```logql
{job="fluentbit", namespace="app"} | json | status > 500
{job="fluentbit", namespace="app"} | json | level = "error"
```

## 6. 配置变更

```bash
# 改 ConfigMap（如 RUST_LOG=debug）后滚动生效
kubectl apply -f k8s/configmap.yaml
kubectl -n app rollout restart deploy/app
```

## 7. 故障排查

| 症状 | 排查 |
|---|---|
| Pod 卡在 Init | `kubectl -n app logs deploy/app -c migrate`——迁移 SQL 报错 |
| readiness 失败 | `kubectl -n app logs deploy/app`——`readiness check failed`；检查 DATABASE_URL |
| Fluent Bit 不工作 | `kubectl -n monitoring logs ds/fluent-bit`；确认 loki service 名与 output 一致 |
| 域名不通 | `kubectl -n app get ingress`；确认 DNS 记录指向 ingress-nginx LoadBalancer |

## 8. 数据库迁移（手动）

```bash
# 生成新迁移（改完 model 后）
DATABASE_URL='postgresql://...' cargo run --bin cli -- migration generate --name add_xxx
# 提交 toasty/ 目录，重新构建镜像
```
