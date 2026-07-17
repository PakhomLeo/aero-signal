# aero-signal 后端模块详设

本文是 Aero Signal 的后端模块与接口职责基线。

## 1. 模块边界

Signal 仅管理微信公众号监测、授权采集登录账号、文章归档和分组 API。后台身份固定为一个系统管理员；`identity` 的多角色、注册、会员能力不在本产品启用。所有模块均以“监测公众号”和“采集登录账号”分离为前提。

| 模块 | 职责 |
| --- | --- |
| administrator | 初始化唯一管理员、登录、改账号/密码、会话审计 |
| collector-login | 固定出口选择、二维码会话、登录态密钥引用、重新登录 |
| connector | `mp_admin`/`weread` capability、probe、文章候选发现 |
| source | 文章链接解析、公众号去重、三篇首次验证、状态管理 |
| heartbeat | 首次锚点、到期判断、空抓延期/降级、可解释事件 |
| daily-preflight | 07:30 一次性检查并固化健康快照 |
| daily-plan | 08:00 根据快照和心跳生成非均匀计划 |
| article-capture | 详情抓取、懒加载图片、正文筛选、资产下载 |
| content-normalizer | Markdown、OCR block 回填、revision、人工编辑 |
| egress | 本机/固定/动态出口、绑定、租约、运行结果 |
| delivery | 分组、Token、articles cursor API、health snapshot API |

### 1.1 工程实现模式

后端模块统一按 `api -> service -> domain -> repository -> adapter/job` 分层实现：

- `api` 层：Axum handler、DTO、session/token 鉴权、幂等键读取、分页 cursor 解析和错误映射。
- `service` 层：业务用例编排、事务边界、job 创建、审计、outbox/change event 写入。
- `domain` 层：状态机、心跳规则、等级降级、文章 identity、图片决策、Token scope 等纯规则。
- `repository` 层：SQLx 查询、事务内写入、唯一约束冲突映射、cursor 查询。
- `adapter` 层：connector、browser-runtime、secret store、object store、OCR、proxy provider、Redis。
- `jobs` 层：长任务 handler，所有超过 2 秒的流程必须可恢复、可重试、可取消、可幂等。

所有模块必须遵守三个工程约束：

1. 状态只能由 domain transition method 变更；SQL 不直接硬改状态。
2. 外部副作用先写 job/lease，再由 worker 执行；HTTP handler 不直接跑抓取、OCR、preflight。
3. 影响下游可见结果的提交必须同时写 revision/change event/outbox 和审计。

## 2. 路由与 API

### 2.1 管理员与设置

```text
GET/PATCH  /api/admin/me                 # 当前唯一管理员；改账号/密码
GET/PATCH  /api/admin/settings/schedule  # 时区、07:30/08:00、工作窗口、图片/OCR阈值
GET/POST/PATCH /api/admin/settings/egress-profiles
POST       /api/admin/settings/egress-profiles/{id}/test
GET/POST/PATCH /api/admin/settings/proxy-providers
```

密码变更必须验证当前密码并撤销其他 session。代理供应商写接口只接受密钥引用或一次性明文输入，响应不回显密钥。

### 2.2 采集登录账号与二维码

```text
GET        /api/admin/collector-accounts
POST       /api/admin/collector-accounts/qr-sessions
GET        /api/admin/collector-accounts/qr-sessions/{ticket}
DELETE     /api/admin/collector-accounts/qr-sessions/{ticket}
POST       /api/admin/collector-accounts/{id}/probe-now
POST       /api/admin/collector-accounts/{id}/reauthorize
PATCH      /api/admin/collector-accounts/{id}
```

创建 QR session 请求包含 `connector` 和可选 `fixed_egress_id`；响应仅包含 `ticket`、`qr_data_url`、`expires_at`、`poll_after_ms`。成功响应的账号信息不包含 credential/token。前端用 `GET` 轮询，不能改为浏览器直接请求第三方二维码或 callback。

### 2.3 监测公众号与文章

```text
POST       /api/admin/sources/validate       # 解析并创建三篇验证 job
POST       /api/admin/sources                 # 仅验证成功后确认创建
GET/PATCH  /api/admin/sources/{id}
POST       /api/admin/sources/{id}/fetch-now
POST       /api/admin/sources/{id}/verify-again
GET        /api/admin/sources/{id}/articles
GET        /api/admin/articles/{id}
PATCH      /api/admin/articles/{id}/content
POST       /api/admin/articles/{id}/reocr
POST       /api/admin/articles/{id}/refetch
```

`validate` 幂等键为输入 canonical URL hash；确认创建使用验证 session id，防止管理员复制一个未验证的解析结果。`refetch` 始终按文章原始 URL 进入详情抓取，不走发现 connector。

### 2.4 计划、分组与消费 API

```text
GET        /api/admin/health-snapshots/latest
POST       /api/admin/daily-plan/rebuild
GET        /api/admin/daily-plan?date=YYYY-MM-DD
GET/POST/PATCH /api/admin/delivery-groups
POST/DELETE /api/admin/delivery-groups/{id}/tokens

GET        /api/articles
GET        /api/articles/{id}
GET        /api/articles/since
GET        /api/sources
GET        /api/service-health/latest
GET        /api/service-health
```

## 3. collector-login 模块

### 3.1 状态机

```text
qr_waiting -> qr_scanned -> finalizing -> active
qr_waiting/qr_scanned -> expired | cancelled | failed
active -> needs_relogin -> qr_waiting
active -> disabled
```

`QrLoginService` 负责 ticket 生命周期，`ConnectorQrAdapter` 负责平台协议，`SecretStore` 负责保存凭据，`BrowserRuntime` 仅在 connector 需要浏览器完成授权时提供隔离 profile。ticket 在 Redis 中保存 `{administrator_session_hash, connector, fixed_egress_id, provider_state_secret_ref, expires_at}`；敏感 state 不得进 PostgreSQL。

最终登录必须使用创建二维码时的固定出口，并调用 `connector.probe()`。探测成功才在事务中创建/更新 `collector_accounts`、`collector_login_sessions`、`collector_egress_bindings`；失败删除临时密钥与 ticket。

## 4. connector 模块

```rust
pub enum ConnectorCapability { QrLogin, DiscoverRecent, FetchFullArticle }
pub enum ProbeStatus { Healthy, Degraded, NeedsRelogin, Unsupported, Failed }

pub trait ConnectorBackend {
  fn capabilities(&self) -> CapabilitySet;
  async fn probe(&self, account: &CollectorAccount) -> Result<ProbeResult>;
  async fn discover_recent(&self, req: DiscoverRequest) -> Result<Vec<ArticleCandidate>>;
}
```

connector 选择条件：账号 `active`、绑定固定出口健康、capability 已启用、未冷却、当日预算未耗尽。任何不满足条件都产生结构化原因，不能静默换到不同平台路径。`weread` capability 未通过集成验收时返回 `connector.unsupported`；LO0/LO1 计划记录为跳过并通知管理员。

## 5. source 与首次验证模块

`SourceParser` 从链接提取文章 identity 和公众号强标识。`SourceValidationService` 在临时 `source_validation_runs` 中写解析结果，调用 connector 获取最多 3 个候选，以文章 identity 去重后并行创建详情任务。它汇总 `0..3` 个结果：

- 至少一个 `ready`：生成 `pending_configuration` source draft；管理员提交 `first_heartbeat_at` 后变为 `active`。
- 零个 `ready`：保持 `needs_action`，带具体的 connector/登录/网络/正文错误。
- 候选少于三篇不是失败；`available_count` 和缺少原因必须显示。

## 6. heartbeat 与日计划模块

`HeartbeatService` 是唯一可计算 `next_heartbeat_at` 的模块。输入为锚点、effective level、窗口、最近成功、连续空抓数和管理员覆盖；输出为新时间、等级变化、原因代码和 `source_heartbeat_event`。

```rust
enum HeartbeatOutcome { NewArticle, EmptySuccess, TechnicalFailure, AccessBlocked, Manual }
```

- `EmptySuccess` 才增加空抓；按 PRD 阈值增加 interval 或降级。
- `TechnicalFailure` 以指数退避创建 retry job，达到上限进入 `needs_action`，不改变等级。
- `NewArticle` 清零计数；允许 L2/L3 创建限期 L1 promotion。
- LO0/LO1 不调用自动降级路径。

`DailyPlanService` 先读取同日成功的 `service_health_snapshot`。没有快照或快照为 `unhealthy` 时不生成可执行计划；`degraded` 时只为健康 connector 生成计划。slot allocator 将窗口按分钟容量、账号串行间隔和全局并发分桶后随机抽样；禁止将 N 个账号等距排为 N 个固定时点。

## 7. preflight 与健康 API 模块

`DailyPreflightService` 每日 07:30 以 `preflight:<date>` 幂等键运行：检查 queue、object store、每个已启用 connector account、绑定固定出口和可用于详情的动态出口。检查只做最小连通/授权 capability 验证，禁止发现文章或下载详情。

它写不可变的 `service_health_snapshots` 和 `service_health_components`。若某单项失败，快照仍然写入。07:45 后 health API 返回该快照；08:00 日计划只读它。管理员的单对象“立即检查”写独立运行记录，不能覆盖该日快照。

## 8. egress 模块

`EgressResolver` 返回 `LocalDirect`、`FixedBinding` 或 `DynamicLease`。固定绑定的切换只能由管理员或 preflight 的连接性失败发起；切换事务将关联会话置为 `needs_relogin`。动态租约由 `proxy_provider` 适配器产生，支持 `static_endpoint/gateway/vendor_api`，统一输出 URL、TTL、用途与安全引用；下载完成或 TTL 到期释放。

错误分类：`network_unreachable` 可尝试下一个健康出口；`authentication_expired` 需重新扫码；`rate_limited/captcha/waf/platform_denied` 进入 `needs_action`，不得轮换出口重试。

## 9. article-capture 与内容模块

`ArticleCaptureService` 以 `(source_id,biz,mid,idx)` 或 canonical URL 取得幂等锁。抓取阶段：`fetch_html -> render_if_lazy -> extract_body -> classify_images -> download_assets -> normalize_markdown -> ocr -> publish_revision`。

`ImageClassifier` 输出 `kept/decorative/too_small/hidden/duplicate/unsupported`。每一个丢弃决定保存阈值、测量值与原因，支持管理员在详情页复核。OCR 失败不回滚文章：文章进入 `ready_with_ocr_warning`，可单独重跑 OCR。人工编辑写 `article_revisions(editor_kind=administrator)`，绝不覆盖抓取版。

## 10. job、错误与审计

| job kind | 幂等键 |
| --- | --- |
| `daily_preflight` | `preflight:<date>` |
| `build_daily_plan` | `daily-plan:<date>` |
| `collector_probe` | `collector-probe:<collector>:<date>` |
| `source_validation` | `source-validate:<url-hash>` |
| `fetch_source` | `source-fetch:<source>:<heartbeat-event>` |
| `fetch_article_detail` | `detail:<article-identity>` |
| `ocr_article_image` | `ocr:<asset-hash>:<ocr-version>` |
| `refetch_article` | `refetch:<article>:<requested-at>` |

所有错误使用统一格式，至少包括 `collector.qr_expired`、`collector.needs_relogin`、`connector.unsupported`、`source.initial_validation_failed`、`egress.network_unreachable`、`crawler.blocked_by_protection`、`image.filtered`、`ocr.failed`、`health.snapshot_unavailable`、`token.scope_denied`。审计和 `safe_context` 只保存错误码、任务 id、状态和脱敏计数。

## 11. 验收重点

- 二维码前端只显示 data URL 和状态，确认后服务端持久化 secret reference；取消/过期不留凭据。
- 添加公众号只验证至多三篇，至少一篇成功才可配置，首次时间是后续心跳锚点。
- 空抓降级与技术失败严格区分；LO0/LO1 永不自动降级。
- 07:45 前生成快照，08:00 计划依赖快照，health API 只暴露脱敏快照。
- 固定出口会话绑定、动态出口详情租约、懒加载图片、OCR block 和 revision 均可测试与追踪。

## 12. 模块级工程实现清单

### 12.1 administrator

实现组件：

- `AdministratorService`：bootstrap 唯一管理员、登录、改账号、改密码、撤销其他 session。
- `PasswordHasher`：Argon2id hash/verify，参数集中配置并可迁移。
- `SessionService`：生成随机 session token，只存 hash；支持过期、撤销、退出其他会话。
- `AdministratorRepository`：保证 `system_administrator.singleton=true` 唯一行。

代码逻辑：

1. bootstrap 从部署 secret 读取初始账号密码，事务内插入唯一行；若已存在则拒绝重复 bootstrap。
2. 登录时按 username 取唯一管理员，校验密码，若 `must_change_password=true` 则只允许进入改密流程。
3. 改密码必须验证当前密码，写新 hash，更新 `password_changed_at`，撤销除当前会话外的所有 session，写审计。

测试重点：重复 bootstrap、弱密码拒绝、改密撤销会话、session hash 不回显、审计不记录明文。

### 12.2 collector-login

实现组件：

- `QrLoginService`：二维码 ticket 生命周期和状态机。
- `ConnectorQrAdapter`：平台二维码协议适配，只返回 data URL、provider state 引用和状态。
- `SecretStore`：保存临时 provider state 与最终 credential/profile 引用。
- `CollectorAccountRepository`：在最终 probe 成功后创建/更新采集账号、登录 session 与固定出口绑定。

代码逻辑：

1. `create_qr_session` 校验管理员 session、connector 和 fixed egress 用途；生成随机 ticket。
2. 调用 adapter 创建二维码，会话敏感 state 写 secret store，ticket 元数据写 Redis，TTL 不超过二维码有效期。
3. `poll_qr_session` 读取 Redis，校验 ticket 所属管理员 session 和固定出口，调用 adapter poll。
4. 状态为 confirmed 时进入 `finalizing`，同一固定出口调用 `finalize_login` 和 `probe`。
5. probe 成功后事务内持久化 `collector_accounts`、`collector_login_sessions`、`collector_egress_bindings`；失败清理临时 secret 和 ticket。

实现模式：ticket 使用 compare-and-set 防止并发 poll 重复 finalize；confirmed 后重复 poll 返回同一 collector account 摘要，不重复保存凭据。

### 12.3 connector

实现组件：

- `ConnectorRegistry`：按 connector name 注册 backend，暴露 capability 和版本。
- `ConnectorBackend` trait：`probe`、`discover_recent`、可选 `create_qr_session/poll/finalize`。
- `ConnectorSelector`：根据 source 等级、账号状态、固定出口健康、capability、预算和冷却选择账号。
- `ProbeRecorder`：将 capability probe 结果写入 collector 状态和 health component。

代码逻辑：

1. 每日 preflight 对 active 账号调用 `probe`，结果归一为 `Healthy/Degraded/NeedsRelogin/Unsupported/Failed`。
2. 计划生成时只读当日 snapshot 和 collector capability，不即时探测。
3. 发现候选文章时，selector 返回一个结构化选择结果；无可用 connector 时写 `connector.unsupported` 或 `collector.needs_relogin`。
4. connector 只负责候选发现，不把登录 token 传给文章详情 URL。

测试重点：未验收 weread 返回 unsupported、mp_admin 不枚举未授权公众号、probe 降级原因可被 health API 脱敏输出。

### 12.4 source 与首次验证

实现组件：

- `SourceParser`：从 URL 提取 `__biz/mid/idx/sn`、canonical URL、样例标题和 source 标识。
- `SourceValidationService`：创建验证 run，调用 connector 发现最多三篇候选，并派发详情抓取 job。
- `SourceRepository`：强标识去重，生成 `pending_configuration` 或 `needs_action`。
- `ValidationResultAssembler`：聚合三篇详情 job 状态，生成前端可展示结果。

代码逻辑：

1. `POST /sources/validate` 以 canonical URL hash 做幂等，已有 run 直接返回。
2. parser 得到强标识后先查 `source_account_ids`；已存在则返回已有 source，并提示不重复创建。
3. connector 发现候选后按 article identity 去重，只取前三篇；每篇创建 `fetch_article_detail` job。
4. 聚合结果时至少一篇 `ready` 才允许确认创建；零篇成功写 `needs_action` 和失败原因。
5. `POST /sources` 只能引用成功验证 run，并要求管理员提交等级、窗口和 `first_heartbeat_at`。

实现模式：验证 run 是临时事实，有过期清理；验证产生的文章若最终未创建 source，按保留期清理孤立资产。

### 12.5 heartbeat 与 daily-plan

实现组件：

- `HeartbeatService`：唯一计算 `next_heartbeat_at` 和等级变化的 domain service。
- `PlanEligibilityService`：根据 health snapshot、source 状态、等级、connector capability 判断今日可执行性。
- `SlotAllocator`：在工作窗口内按容量、账号串行间隔和全局并发随机抽样。
- `DailyPlanService`：以 `daily-plan:<date>` 幂等创建计划和 `fetch_source` job。

代码逻辑：

1. 抓取完成后只把 outcome 交给 `HeartbeatService::apply_outcome`，返回新等级、空抓计数、下次时间和 reason_code。
2. `EmptySuccess` 才增加空抓；`TechnicalFailure` 创建 retry；`AccessBlocked` 进入 `needs_action`。
3. 08:00 build plan 必须读取同日成功快照；没有快照或 unhealthy 时不创建可执行计划。
4. 重建计划只影响 `planned` 且未开始的行；已 running/succeeded/failed 的计划不可覆盖。

测试重点：时间冻结、LO0/LO1 不降级、L2/L3 临时提升、管理员手动覆盖、计划重建不覆盖已执行任务。

### 12.6 daily-preflight 与 health API

实现组件：

- `DailyPreflightService`：执行一次性检查并生成不可变 snapshot。
- `HealthComponentChecker`：queue、object store、collector、connector、fixed egress、dynamic egress 子检查。
- `HealthSnapshotRepository`：`snapshot_date` 唯一，成功写入后禁止 update。
- `HealthApiPresenter`：脱敏 DTO，隐藏 proxy endpoint、账号真实 ID、cookie/token 和原始错误。

代码逻辑：

1. job 使用 `preflight:<date>` 幂等键，重复触发返回已有 job/snapshot。
2. 子检查并发执行，但每个检查有独立 timeout；单项失败不阻止 snapshot 写入。
3. overall status 由组件状态归纳：核心依赖不可用为 unhealthy，部分账号或出口不可用为 degraded。
4. 管理员“立即检查”写独立 run record，不更新当天 snapshot。

实现模式：snapshot DTO 版本化；health API 对 API Token 只校验 `service_health:read` scope，不暴露后台管理细节。

### 12.7 egress

实现组件：

- `EgressResolver`：按用途返回 local direct、fixed binding 或 dynamic lease。
- `ProxyProviderAdapter`：支持 static endpoint、gateway、vendor API。
- `DynamicLeaseManager`：申请、续约、释放和过期回收。
- `EgressPolicy`：把网络错误、认证失效、平台限制区分成不同决策。

代码逻辑：

1. 登录和发现请求只能解析到 fixed egress 或 local direct；详情和资产下载只能解析到 dynamic egress 或 local direct。
2. fixed egress 连接性失败时，preflight 可按优先级切换并把相关 collector 标记 `needs_relogin`。
3. dynamic egress 在任务开始时申请 lease，任务结束释放；worker 崩溃时由 TTL 回收。
4. `network_unreachable` 可尝试下一个健康出口；`captcha/waf/platform_denied` 不轮换，直接 `needs_action`。

测试重点：用途约束、lease 释放、供应商 API 失败、平台限制不触发轮换。

### 12.8 article-capture

实现组件：

- `ArticleIdentityResolver`：解析 `(source_id,biz,mid,idx)`，缺失时使用 canonical URL。
- `ArticleFetchClient`：HTTP 优先抓 HTML，必要时调用 browser-runtime。
- `BodyExtractor`：提取正文 DOM、标题、作者、发布时间、候选图片和 block anchor。
- `ImageClassifier`：图片保留/丢弃决策。
- `AssetDownloader`：下载保留图片，按 hash 去重并写对象存储。

代码逻辑：

1. job 以 article identity 获取幂等锁；重复发现只更新 `article_links.discovered_at`。
2. HTML 不完整、图片只在懒加载属性中或正文容器未出现时，调用 browser-runtime 渲染并滚动正文容器。
3. 每张候选图片都写 `article_image_decisions`，包含测量尺寸、可见性、重复率、原因。
4. 下载成功的正文图片写 `article_assets`；正文外资源不进入 manifest。
5. 抓取结果交给 content-normalizer，不直接发布对外 revision。

实现模式：raw HTML 和截图只作为调试资产，有 retention policy；抓取失败使用结构化错误，不伪造空文章。

### 12.9 content-normalizer 与 OCR

实现组件：

- `MarkdownBuilder`：把正文 block、图片 asset 引用和 OCR anchor 组合成 `content_md`。
- `OcrJobPlanner`：为每张保留正文图片创建 `ocr_article_image` job。
- `RevisionService`：创建 article revision、更新 current revision、写 change event。
- `ManualEditService`：后台人工编辑、恢复版本、重新 OCR、按链接重抓。

代码逻辑：

1. 正文 HTML 先转 block AST，再生成 Markdown，避免字符串拼接破坏结构。
2. OCR 按图片 block anchor 回填，低置信度标注为图片文字块，不伪装为原文。
3. OCR 失败不回滚文章；文章可为 `ready_with_ocr_warning`，后续单独重跑失败图片。
4. 内容 hash 或 asset manifest hash 变化时才创建新 revision；人工编辑必须保留 crawler 版本。
5. 每个对外可见变更都写 `article_change_events`。

测试重点：OCR anchor 稳定、重跑 OCR 幂等、人工编辑不覆盖抓取版、资产变化触发增量事件。

### 12.10 delivery

实现组件：

- `DeliveryGroupService`：分组、绑定 source、停用 source。
- `ApiTokenService`：签发一次性明文 Token、hash 存储、scope、IP allowlist、过期和撤销。
- `ArticleCursorService`：`/api/articles/since` 基于 `change_sequence` 增量读取。
- `HealthDeliveryService`：返回脱敏 health snapshot。

代码逻辑：

1. Token 创建时只返回一次明文；之后只能看到 token 前缀、scope、有效期和状态。
2. 文章列表按 group source 过滤；详情接口必须同时校验文章所属 source 在 group 内。
3. since cursor 使用单调 `change_sequence`，返回 event_id、article_id、revision、change_type、schema_version。
4. 下游重复消费同一 event 必须幂等；撤回和删除通过 tombstone/withdrawal event 表达。

实现模式：API DTO 版本化；Token scope 与 IP allowlist 在 middleware 中校验；调用日志只保存 token id、scope、状态码、耗时和脱敏计数。

### 12.11 job、错误、审计与可观测性

实现组件：

- `JobHandlerRegistry`：注册所有 job kind 和 handler。
- `RetryPolicy`：按错误类型决定 retry/non_retryable/needs_action。
- `AuditWriter`：服务层统一写审计。
- `SafeContext`：job 和 attempt 的脱敏上下文。
- `TelemetryLayer`：tracing span、metrics、slow job 告警。

代码逻辑：

1. API 创建长任务时先写 job，再返回 job id；前端通过 job id 轮询状态。
2. handler 开始时获取 lease，读取最新业务状态，若对象已暂停、撤销或过期则安全 skipped。
3. retryable 错误写 `retry_wait` 和下一次运行时间；non-retryable 错误写 failed/needs_action。
4. 审计写在业务事务内；外部 provider 调用日志只写 provider、模型、输入规模、输出规模、耗时和错误码。
5. 所有日志和 metrics 使用稳定资源 id，不写正文、cookie、token、proxy endpoint 或二维码原始内容。

### 12.12 代码审核执行补充

- Heartbeat 使用 `event_key + source.version` 做 CAS 幂等；投递组使用聚合事务写入 metadata 与 source bindings。
- Worker 的结果、进度、状态转换和 lease release 必须绑定当前 worker，避免过期 worker 覆盖新 worker。
- 每次 job acquisition 使用 UUID 后缀的 lease owner token，避免同名 worker 重启后旧执行继续写入。
- 手动抓取、refetch、OCR rerun 支持客户端 request id；文章编辑支持 expected revision。
- Batch 6 将 service 的输入校验、时间/等级决策、响应映射和 article PostgreSQL 枚举编解码提取为内部模块；不改变 API、DTO、job kind、状态字符串或错误码。
- Worker 读取既有 `execution_timeout_at` 并在 handler 超时后执行 cancel/abort；`mp_admin` 统一使用不回显原始请求 URL 的网络错误。
