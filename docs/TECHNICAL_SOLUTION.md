# aero-signal 技术方案

本文是 Aero Signal 的技术架构与实现基线。

## 1. 架构边界

Signal 是单租户、唯一管理员的服务器后台。它维护监测公众号、采集登录账号、每日计划、文章版本、图片资产、分组 Token 和每日健康快照。身份模块在本产品仅启用 singleton administrator 模式；消费 API 使用分组 Token，不使用后台角色系统。

```text
Vue Admin
  -> Axum Admin/API
  -> Signal services / SQLx / PostgreSQL
  -> Redis: QR ticket、lease、任务队列、短期限流
  -> browser-runtime: 授权扫码和懒加载页面渲染
  -> platform-connectors: mp_admin / weread capability adapter
  -> crawler-capture: 详情、正文图片、Markdown
  -> OCR provider via provider-hub
  -> MinIO/S3: 图片、原始快照、受控调试资产
```

不在 Signal 内实现反防护规避；平台验证码、登录墙、WAF 和权限不足统一转 `needs_action`。

## 2. 核心技术决策

| 领域 | 决策 |
| --- | --- |
| 服务 | Rust stable / Axum / SQLx / PostgreSQL |
| 异步任务 | `job-pipeline`，DB/Redis lease 与幂等键 |
| 登录态 | `browser-runtime` persistent profile + encrypted secret reference；不存明文 cookie |
| 二维码 | connector adapter 创建一次性 ticket；Redis 保存短期状态；前端显示 data URL |
| 连接器 | `platform-connectors` capability-first，不假定未公开接口稳定 |
| 正文抓取 | `crawler-capture`，HTTP 优先、浏览器仅作懒加载渲染兜底 |
| OCR | `provider-hub`，每张已保留正文图片建立 OCR job 与位置映射 |
| 资产 | MinIO/S3，按 content hash 去重，文章只引用受控资产 |
| 出口 | 登录/发现固定出口；详情/图片动态出口；本机出口是默认回退 |

### 2.1 工程实现总蓝图

Signal 按 hexagonal architecture 组织代码：API 层只做鉴权、DTO 校验、幂等键读取和错误映射；application service 编排业务流程；domain 层保存状态机、等级、心跳、文章 identity、图片决策等纯规则；repository 层只负责 SQLx 查询与事务；adapter 层封装 connector、browser-runtime、对象存储、OCR provider、secret store、Redis 和 proxy provider。

推荐目录：

```text
service/src/
  api/
    admin/                  # 后台路由、request/response DTO、权限和幂等 middleware
    delivery/               # 分组 Token API、cursor API、health API
  domain/
    administrator.rs
    collector.rs
    connector.rs
    source.rs
    heartbeat.rs
    plan.rs
    article.rs
    asset.rs
    delivery.rs
  service/
    administrator_service.rs
    qr_login_service.rs
    connector_registry.rs
    source_validation_service.rs
    heartbeat_service.rs
    daily_preflight_service.rs
    daily_plan_service.rs
    article_capture_service.rs
    content_normalizer.rs
    egress_service.rs
    delivery_service.rs
  repository/
    *_repo.rs               # SQLx query + transaction boundary
  jobs/
    handlers.rs             # job-pipeline handler registry
    daily_preflight.rs
    build_daily_plan.rs
    source_validation.rs
    fetch_source.rs
    fetch_article_detail.rs
    ocr_article_image.rs
  adapters/
    connectors/
    browser_runtime/
    object_store/
    ocr_provider/
    secret_store/
    proxy_provider/
  dto/
  error.rs
  audit.rs
  idempotency.rs
  clock.rs
```

核心代码规则：

- 所有跨表写入必须从 service 层开启事务，并在同一事务中写业务事实、审计、outbox/change event；adapter 不允许直接写主业务表。
- 每个长流程拆成 `command -> job -> domain decision -> repository commit -> event`。同步 API 只创建或查询 job，不在 HTTP 请求内完成抓取、OCR、preflight 或计划重建。
- 状态转换只能通过 domain 方法完成，例如 `CollectorAccount::mark_needs_relogin`、`HeartbeatRule::apply_outcome`、`Article::publish_revision`；repository 不直接拼状态字符串。
- DTO 与 domain model 分离。API 输入先转 command，command 再由 service 转 domain；数据库 row 不直接返回给前端。
- 所有时间依赖通过 `Clock` trait 注入；心跳、07:30/08:00 调度、二维码过期和 Token 过期必须可时间冻结测试。
- 所有外部能力通过 trait + fake adapter 测试，不在单元测试里真实访问微信、代理、对象存储或 OCR 服务。

### 2.2 横切工程模式

| 场景 | 实现模式 | 代码要求 |
| --- | --- | --- |
| 写请求幂等 | `Idempotency-Key` + command hash + resource key | 重复请求返回同一 job 或同一资源，不重复建二维码、source、plan、article |
| 并发控制 | PostgreSQL transaction + unique constraint + Redis/DB lease | source 验证、文章详情抓取、每日计划、OCR 均以业务唯一键加锁 |
| 状态机 | domain enum + transition method | 禁止 API 或 SQL 直接从任意状态跳到目标状态 |
| 错误处理 | typed error + `module.reason_code` | 前端、审计、safe_context、health snapshot 使用同一错误码 |
| 审计 | service 层统一 `AuditWriter` | 记录动作、资源、结果、脱敏计数；不记录正文、凭据、代理密钥 |
| 下游变更 | article revision + change event + outbox | 新增、重抓、编辑、撤回和资产变化都产生可重放事件 |
| provider 调用 | trait adapter + call log + timeout/cancel token | OCR、对象存储、browser-runtime 和 proxy provider 均有 fake、超时、限流、取消测试 |
| 可观测性 | tracing span + metrics + job progress | 每个 job 带 `job_id/source_id/collector_id/article_id`，日志只写脱敏字段 |

## 3. 二维码登录技术流程

实现借鉴常见二维码登录服务的会话形态（服务层 + 可替换 QR provider），不复制其中可能变化的平台请求细节。

```text
POST /api/admin/collector-accounts/qr-sessions
  -> choose fixed egress + connector
  -> connector.create_qr_session()
  -> store encrypted transient state in Redis by random ticket, TTL
  -> return {ticket, qr_data_url, expires_at, poll_after_ms}

GET /api/admin/collector-accounts/qr-sessions/{ticket}
  -> connector.poll(ticket state)
  -> waiting | scanned | confirmed | expired | failed | cancelled
  -> on confirmed: connector.finalize on same egress
  -> save credential secret reference + profile metadata + binding atomically
```

规则：

- data URL 由服务端读取二维码二进制或将扫码 URL 转 QR SVG 生成，解决第三方图片 hotlink 问题；不把二维码 URL 直接暴露给前端。
- `ticket` 绑定当前管理员 session、connector 和固定出口，TTL 不超过二维码有效期；取消/过期立即删除。
- 前端轮询间隔 2–3 秒，绝不轮询或展示凭据。状态为 `confirmed` 后停止轮询。
- 最终凭据以 `secret_ref` 存入密钥后端，`collector_login_sessions` 只记录加密引用、过期时间、外部账号展示名和绑定出口。
- 固定出口切换使会话失效，必须重新扫码；不能在已有登录会话上静默换出口。

## 4. connector 设计与能力门禁

```rust
trait WechatConnector {
    fn channel(&self) -> WechatChannel;
    async fn create_qr_session(&self, ctx: LoginContext) -> Result<QrSession>;
    async fn poll_qr_session(&self, session: QrSession) -> Result<QrPollResult>;
    async fn finalize_login(&self, session: QrSession) -> Result<LoginCredentialRef>;
    async fn probe(&self, account: CollectorAccount) -> ProbeResult;
    async fn discover_recent(&self, source: Source, account: CollectorAccount, limit: u8) -> Result<Vec<ArticleCandidate>>;
}
```

`mp_admin` 与 `weread` 是不同 adapter。每个 adapter 需声明 `qr_login`、`discover_recent`、`full_article` 三项 capability；每日 preflight 真实验证已启用 capability。`weread` 在没有授权、可复现且已验收的集成前保持 `off`，不能因等级为 LO0/LO1 而假装可用。`mp_admin` 仅用于被授权管理的账号能力，不能设计为任意第三方公众号目录。

文章详情抓取不携带登录 token 到 `mp.weixin.qq.com/s` URL；connector 只负责文章候选发现，详情服务独立读取 canonical URL。

## 5. 计划与健康快照

### 5.1 每日编排

```text
07:30 daily_preflight job
  -> fixed egress connectivity
  -> dynamic egress connectivity
  -> each active collector login/capability probe once
  -> worker/queue/object-store readiness
  -> persist immutable health snapshot
07:45 snapshot publication deadline
08:00 build_daily_plan
  -> select source.next_heartbeat_at <= day_end
  -> exclude paused/L4/unverified/no compatible healthy collector
  -> allocate weighted-random execution times in work windows
  -> create idempotent source fetch jobs
```

计划分配使用随机加权采样，不采用账号序号等距切片：先按窗口长度和剩余并发容量分配桶，再在每桶内抽样时间；同一采集账号的最低间隔、任务最大并发和每日预算为硬约束。随机仅用于均衡运营负载，不作为规避机制。

### 5.2 心跳算法

`next_heartbeat_at` 从管理员设置的 `first_heartbeat_at` 和 effective interval 计算。成功发现新文章时清零空抓计数；成功但无新文章时增加计数并按 PRD 表延期或降级；技术故障不增加空抓。规则与每次决定的输入/输出保存到 `source_heartbeat_events`，使“为何今天抓/为何未抓”可解释。

## 6. 出口与代理模型

```text
egress_profiles
  fixed: local | static proxy | ISP/static vendor
  dynamic: local | static list | rotating gateway | vendor allocation API
collector_egress_bindings
  collector account -> one active fixed egress
article fetch lease
  task -> dynamic egress lease -> release/expire
```

- 固定出口只能用于二维码登录、登录态维持和 connector 发现。预检检测到网络不可达时，按优先级选择下一个固定出口并将依赖会话标记 `needs_relogin`。
- 动态出口只能用于详情/图片任务。供应商 API 适配仅处理“申请/释放端点、TTL、认证、地区、响应映射”；认证密钥使用 `secret_ref`。
- 任务网络错误可选下一个健康动态出口；平台限制、验证码、授权失效不触发轮换。
- 所有探测在 preflight 执行一次；单次任务失败只记录该任务和该出口的运行结果，避免重复探测风暴。

## 7. 文章规范化、图片与 OCR

1. 规范化 URL，并提取 `__biz/mid/idx/sn` 作为优先文章身份。
2. 获取 HTML，解析正文容器。若发现图片只在懒加载属性中或 DOM 未完整，调用 browser-runtime 进行受限渲染、正文滚动和 DOM 提取。
3. `ImageClassifier` 对候选图片进行 DOM 位置、可见性、重复率、文件类型、实际宽高、面积和 hash 判断；将每次“保留/剔除”及原因保存。
4. 下载保留图片至对象存储。对每张图片创建 OCR job；返回文本以 block anchor 插入 Markdown，保留置信度与原图引用。
5. 输出 `content_md`、文章 hash、asset manifest hash；仅在内容/资产/人工编辑发生变化时创建 revision 与 change event。

原始 HTML 仅作为受控的调试快照，有独立保留期；对外 API 只输出 Markdown 和受控图片 URL。

## 8. API 与安全

### 8.1 代码审核执行补充

- Crawler page/asset/CDP 目标统一通过 SSRF guard，HTTP redirect 逐跳重新授权并保留响应大小上限。
- ServiceConfig、connector runtime state、safe_context 和 job error 均禁止输出凭据、cookie、token、原始二维码或代理密码。
- 前端 API client 统一 timeout/AbortSignal，401 清理本地 session；QR refresh 先 cancel-old，文章编辑带 revision guard。
- Batch 6 service/repository 纯逻辑拆分为内部模块，编排仍保持 `api -> service -> domain -> repository -> adapter/job` 边界。
- Worker 超时通过 `execution_timeout_at` 竞争 handler；mp_admin 网络失败使用稳定脱敏消息，CDP 在 redirect 后重新校验有效 URL。

后台 API 只要求 singleton administrator session；所有写请求支持 idempotency key。消费 API 由分组 Token 保护，除了文章接口新增：

```text
GET /api/service-health/latest
GET /api/service-health?date=YYYY-MM-DD
```

响应 DTO 必须版本化，健康快照不输出 proxy endpoint、cookie、token、账号真实 ID 或原始错误文本。Token 为 hash 存储，创建后只回显一次。后台密码用 Argon2id，管理员变更账号/密码、二维码登录确认、出口绑定与 API 配置写审计。

## 9. 测试与验收策略

- QR adapter mock 覆盖生成、扫描、确认、过期、取消、最终验证失败；确认只保存 secret reference。
- 同一二维码 ticket 不能被另一个管理员 session 或出口使用。
- 首次验证最多传入并保存三篇不同候选；至少一篇成功才允许配置。
- 07:30 preflight 一天只创建一个成功快照；08:00 计划依赖该快照。
- 心跳空抓、失败、临时提升、自动降级和 L0/L1 不自动降级均有时间冻结测试。
- 懒加载图片、装饰图、小图标、OCR 定位、人工编辑和文章 identity 去重都有 fixture。
- `/api/service-health` 的 Token scope、脱敏、日期查询和快照不可变性都有契约测试。

## 10. 未来风险、遗漏问题与解决方案

| 问题 | 风险 | 解决方案 |
| --- | --- | --- |
| connector 能力不稳定 | `mp_admin` 或 `weread` 的可用能力可能随平台变化失效，导致计划大量跳过或误判为空抓 | connector registry 必须版本化 capability；preflight 保存 probe 证据和 reason_code；每个 connector 有 contract fixture 和灰度开关；未通过 probe 的账号只进入 `needs_action`，不参与计划 |
| 文章 DOM 与懒加载规则变化 | 正文抽取、图片筛选或 OCR block 锚点可能因页面结构变化而错位 | `ArticleExtractor` 按多策略实现：静态解析、懒加载属性扫描、browser-runtime 渲染兜底；保留 extractor version、raw debug snapshot、image decision；新增 DOM fixture 回归集 |
| 下游增量消费漏事件 | 编辑、重抓、图片变化、撤回若只更新文章表，下游消费方可能无法感知 | 引入 `article_change_events` + delivery outbox；所有影响对外结果的事务必须同时写 revision/change/outbox；`/api/articles/since` 以单调 sequence 和幂等 event_id 返回 |
| 每日计划与手工操作冲突 | 管理员在 08:00 后改等级、暂停或重建计划，可能与已创建 job 互相覆盖 | daily plan 使用不可变计划行；重建只影响未开始任务；source 规则变更写新 heartbeat event；job 开始前二次读取 source 状态，暂停或 L4 则安全跳过 |
| 二维码与凭据泄漏 | ticket、二维码 URL、临时 cookie、storage_state 若进入日志或数据库会形成高风险泄漏 | QR state 只存 Redis/secret store；tracing 增加敏感字段 redact layer；集成测试扫描日志和 DB fixture，禁止出现 cookie、token、二维码原始 URL、proxy password |
| 代理/出口语义被误用 | 固定出口和动态出口若被混用，可能破坏登录态或被误认为自动规避限制 | `EgressResolver` 以用途枚举校验 `login/discover/article_detail/asset_download`；数据库 `allowed_purposes` 加约束；平台限制错误不触发动态出口轮换 |
| 健康快照不可变与实时状态冲突 | 运营人员可能误解“今日快照”和“立即检查”的差异，导致认为系统状态不一致 | API 明确返回 `snapshot_generated_at` 与 `latest_check_at`；后台分开展示“今日快照”和“单项检查记录”；单项检查只写 run record，不覆盖 snapshot |
| OCR 成本和延迟不可控 | 图片多或 OCR provider 慢会拖慢文章 ready 时间，并造成成本不可预期 | OCR 进入独立 job 队列，按 provider 配额限流；文章可先进入 `ready_with_ocr_warning`；设置页暴露阈值、预算、重跑范围；provider call log 记录输入规模和费用估算 |
| 数据保留与合规删除不足 | 原始 HTML、截图、图片、OCR 文本和下游事件若没有保留策略，后期清理困难 | 为 raw snapshot、asset、audit、api log、change event 分别定义 retention policy；撤回使用 tombstone/change event 传播；物理清理前做引用检查并写审计 |
| 单管理员共享账号审计粒度有限 | 多人共用后台时无法识别具体自然人，问题追溯只能到会话 | 文档和 UI 明确共享账号限制；会话保存 ip/user-agent hash；关键操作要求重新输入密码或二次确认；如未来需要个人归因，应独立升级为多管理员模式而不是偷偷加角色字段 |
