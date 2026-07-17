# OpenAPI 与消费 API 契约冻结说明

面向 Paddock / 内部服务对接。首期**不以自动生成 OpenAPI 为硬门槛**；以本仓库已实现路由与集成测试为契约源。

## 冻结范围（消费 API）

基路径：`/api`（Bearer API Token）。

| 方法 | 路径 | Scope | 说明 |
| --- | --- | --- | --- |
| GET | `/articles` | `articles:read` | 组内文章列表 |
| GET | `/articles/{id}` | `articles:read` | 文章详情（当前 revision） |
| GET | `/articles/since` | `articles:read` | 复合 cursor 增量（`change_sequence`） |
| GET | `/service-health/latest` | `service_health:read` | 脱敏日快照 |
| GET | `/service-health?date=YYYY-MM-DD` | `service_health:read` | 按日查询不可变快照 |

约束：

- Token 绑定 `delivery_group`；文章接口仅返回组内 `source_accounts`。
- Health 接口不受文章来源过滤，但**永不**返回 proxy endpoint、cookie、token、账号真实 ID、原始错误正文。
- `since` 使用 `change_sequence`（及文档约定的复合 cursor），禁止仅用时间戳。
- 明文 Token **仅创建时回显一次**；之后只存 hash。

契约测试：`service/tests/delivery.rs`。

## 管理 API

基路径：`/api/admin`（管理员会话 Cookie/Bearer）。变更类操作写审计；错误码见 [错误码表.md](./错误码表.md)。

## 生成 OpenAPI（可选后续）

当前未接入 `utoipa` / 导出 JSON。若需机器可读契约：

1. 在 `signal-service` 为消费路由增加 OpenAPI 注解或手写 `openapi.yaml`；
2. 将生成物提交至 `docs/api/openapi-delivery.yaml` 并在发版 PR 中 diff；
3. **冻结规则**：破坏性变更（删字段、改 cursor 语义、放宽脱敏）必须升 `schema_version` 并通知 Paddock。

在生成流水线落地前，以本文件 + 集成测试为权威。
