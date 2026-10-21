# Aero Signal：Agent 接手与开发指南

本文是本仓库的 AI/开发者单一接手入口。开始修改代码前完整阅读本文，并按任务继续阅读相应专题文档。本文描述的是 2026-07-17 的代码状态；代码与本文冲突时，先验证代码和测试，再同步修正文档。

## 1. 项目是什么

Aero Signal 是单租户、唯一系统管理员的微信公众号采集与归档后台。它负责：

1. 管理采集登录账号、二维码授权和出口绑定。
2. 添加并验证需要监测的公众号。
3. 按心跳等级和每日健康快照生成采集计划。
4. 发现候选文章、判断是否为新文章并执行持久任务。
5. 提取正文、筛选图片、执行 OCR、保存文章版本。
6. 通过分组 Token 和增量游标向下游提供文章与健康状态。

它不是爬虫绕过工具，也不是公众号搜索引擎。`mp_admin` 只能处理当前管理员已授权管理的公众号。

## 2. 必读顺序

按任务最小化阅读，但不要跳过产品和安全边界：

1. 本文。
2. `docs/PRD.md`：产品目标、业务流程和验收口径。
3. `docs/TECHNICAL_SOLUTION.md`：架构、技术决策和横切模式。
4. `docs/BACKEND_MODULES.md`：服务、接口、状态和 Job 职责。
5. 数据变更再读 `docs/DATABASE_DESIGN.md` 与 `migrations/README.md`。
6. API 变更再读 `docs/api/OpenAPI与契约冻结.md` 和 `docs/api/错误码表.md`。
7. 部署变更再读 `docs/部署说明.md`。
8. 采集链路问题再读 `SIGNAL_REPAIR_LOOP_PLAN.md` 的执行结果。

## 3. 不可突破的产品与安全边界

- 系统只有一个 `system_administrator`。不要引入注册、普通用户、角色、会员或计费体系。
- “监测公众号”与“采集登录账号”是不同实体，数据库、服务、API 和 UI 中都不得混用。
- 采集账号只能由管理员授权；凭据只保存 SecretStore 引用，明文 Cookie、Token、二维码 ticket、storage state 和代理密码不得进入普通数据库、日志、审计或 Job safe context。
- 不实现验证码/WAF 绕过、鼠标模拟、设备指纹轮换、MITM、未授权登录或平台限制后的自动换出口继续请求。
- `mp_admin` 不能枚举任意第三方公众号；`weread` 在没有经过授权和验收前保持不可用/显式 unsupported。
- 登录/发现使用固定出口；文章详情/媒体使用动态或本机出口。网络故障可以按策略处理，平台限制不得触发自动规避。
- 所有 URL 抓取必须保留 SSRF 防护、逐跳重定向校验、响应大小上限和超时。
- 单元与集成测试默认使用 fake/fixture，不得访问真实微信、代理、S3 或 OCR。

## 4. 架构总览

```text
Vue Admin / Delivery Client
          │
          ▼
Axum API（鉴权、DTO、幂等键、错误映射）
          │
          ▼
Application Service（业务编排、事务边界、审计）
          │
    ┌─────┼──────────────┐
    ▼     ▼              ▼
 Domain  Repository     Job Pipeline
 规则/状态 PostgreSQL/内存  lease/retry/cancel
          │              │
          └──────┬───────┘
                 ▼
 Adapter/Connector（微信、浏览器、S3、OCR、Secret、代理）
```

依赖方向遵循 `api -> service -> domain/repository -> adapter/job`。API 层只做鉴权、输入校验和输出映射，超过约 2 秒的操作必须创建 Job。

`service/src/app.rs` 是 composition root：选择 memory/postgres 仓储，装配 Adapter、Service、共享限流器和 Job registry。新增依赖时优先从这里注入，不要在业务方法内部创建全局客户端。

## 5. Workspace 模块职责

### 5.1 `service/`：主应用

| 路径 | 职责 | 修改注意 |
| --- | --- | --- |
| `src/api/admin/` | 管理后台路由、Session 鉴权、DTO | 不执行抓取/OCR；长操作只入队 |
| `src/api/delivery/` | 分组 Token API、文章游标、健康接口 | 保持 scope、分页和脱敏契约 |
| `src/domain/` | 管理员、账号、来源、文章、计划、健康等规则 | 状态转换应封装为 domain 方法 |
| `src/service/` | 用例编排、策略和事务语义 | 业务判断集中在此，不下沉到 API/SQL |
| `src/repository/` | trait、内存实现和 PostgreSQL 实现 | 两种实现应保持相同行为 |
| `src/jobs/` | Job handler 与注册 | 新 kind 必须注册真实 handler，不能假成功 |
| `src/adapters/` | Secret、QR ticket、代理和运行时适配 | 所有外部实现提供 fake/超时/错误测试 |
| `src/app.rs` | 应用装配 | HTTP 与 Worker 必须共享一致依赖 |
| `src/scheduler.rs` | 07:30/08:00 调度入口 | 时间逻辑使用 `Clock` |

重要 Service：

- `administrator.rs`：唯一管理员 bootstrap、登录、改密和 Session。
- `qr_login.rs`：二维码状态机、固定出口和凭据引用落库。
- `source_validation.rs`：URL/短链解析、最多三篇初始验证、确认来源。
- `article_capture.rs`：候选发现、详情、正文/资产提交、回填与重试。
- `heartbeat.rs`：新文章、空抓、技术失败、平台限制对应的心跳状态转换。
- `daily_preflight.rs` / `daily_plan.rs`：每日健康快照和非等间隔计划。
- `content_normalizer.rs` / `markdown_builder.rs` / `ocr_planner.rs`：正文、图片锚点与 OCR。
- `revision.rs`：revision、change event、delivery outbox 一致性。
- `request_limiter.rs`：collector、egress、source、host、purpose 多维令牌桶。
- `capture_failure.rs`：跨任务边界的结构化错误分类。

### 5.2 `crates/server-core`

共享 HTTP 基础：`ServiceConfig`、`Clock`、`AppError`、鉴权提取器、trace id、健康路由、分页和幂等工具。这里只放跨应用且稳定的基础能力，不放 Signal 业务规则。

### 5.3 `crates/job-pipeline`

提供 Job 类型、PostgreSQL/内存 Repository、lease、worker、超时、取消、重试、进度和 safe context 白名单。

当前 Job kinds：

```text
daily_preflight
build_daily_plan
collector_probe
source_validation
source_validation_aggregate
fetch_source
history_backfill
fetch_article_detail
retry_article_assets
ocr_article_image
refetch_article
retention_cleanup
```

新增 Job 必须同时完成：kind 常量、幂等键、payload、handler、registry、repository 行为和定向测试。

### 5.4 `crates/platform-connectors`

连接器以 capability-first 方式工作。`backend.rs` 定义 trait，`registry.rs` 根据环境注册实现，`selector.rs` 按能力选择账号。

- `fake.rs`：本地测试实现。
- `mp_admin/`：受授权账号的二维码/发现协议与分页解析。
- `weread.rs`：未验收能力显式 unsupported，不要静默假成功。

连接器负责候选发现，不负责把登录 Token 拼进公开文章 URL。候选证据必须脱敏，只保留 connector、页码/cursor、强身份、标题/作者/时间/封面、摘要 hash 和 observed_at。

### 5.5 `crates/crawler-capture`

文章采集底座：

- `identity.rs`：canonical URL 和 `source + biz + mid + idx` 强身份。
- `capture.rs`：有界 HTTP 抓取、显式重定向、可选浏览器兜底。
- `egress_guard.rs`：SSRF 与私网/元数据地址防护。
- `extractor.rs`：DOM allowlist、语义 HTML/Markdown 和媒体占位。
- `asset_downloader.rs` / `classifier.rs`：媒体下载、实际尺寸、hash 去重和决策。
- `browser.rs`：受限 CDP 懒加载渲染。
- `object_store.rs`：S3/MinIO 与 FakeObjectStore。

不要用正则整体删除 HTML 标签；不要因为单张图片失败而丢弃正文。

### 5.6 `crates/provider-hub`

OCR Provider trait、配置、结构化错误和 Fake Provider。真实 Provider 尚需独立接入和验收；production 不允许使用 Fake OCR。

### 5.7 `web/`

Vue 3 管理后台，包含登录/改密、来源、采集账号、二维码、Job、文章、交付和设置页面。

- `src/api/client.ts`：统一 HTTP、超时、401 处理。
- `src/api/types.ts`：前后端契约类型。
- `src/stores/auth.ts`：管理员 Session 状态。
- `src/views/`：页面级编排。

后端 DTO 改动时同步更新 TypeScript 类型和错误标签；未改前端时不要运行不必要的前端全构建。

### 5.8 `migrations/`

SQLx 迁移只追加，不修改已经应用的文件。数据模型覆盖管理员、采集账号、出口、来源、心跳、文章、资产、revision、健康、计划、交付和保留策略。

跨表可见性变更必须保证事务原子性，尤其是文章 + revision + change event + outbox。

## 6. 核心处理流程

### 6.1 添加监测公众号

```text
POST validate
→ SourceParser 校验直接微信 URL
→ 腾讯短链才进入有界解析；逐跳 SSRF 校验
→ canonical URL 清理敏感参数并提取 biz/mid/idx/sn
→ 创建 source_validation Job（幂等）
→ 发现并去重，最多选择 3 篇
→ 每篇创建 fetch_article_detail
→ aggregate 等待全部详情完成
→ 至少一篇成功才允许确认来源和规则
```

`input_url`、Job payload、数据库和日志中不得出现 `uin`、`key`、`pass_ticket`、`appmsg_token`、Cookie 或 Token。

### 6.2 日常发现与新文章判断

```text
fetch_source
→ 运行时二次检查 paused/L4
→ 解析授权采集账号和 connector
→ list 请求预算
→ 有界分页发现候选
→ 强身份规范化、批内去重
→ 查询已有文章 + 详情 Job 幂等键
→ 只为真正未处理候选创建详情 Job
```

候选非空不等于发现新文章。只有详情成功提交新文章或符合规则的新 revision 后才能记录 `found_new=true`。全部候选都已存在才是“成功但无新文章”。技术失败、凭据失效和平台限制不能计为空抓。

### 6.3 详情、正文和资源

```text
fetch_article_detail
→ detail 多维请求预算
→ HTTP 抓取；正文不完整时才使用受限浏览器
→ DOM allowlist 清洗和语义 Markdown
→ 图片候选分类
→ 每个媒体独立预算、下载和对象存储
→ 单资源失败记录 download_failed，正文继续
→ 原子提交 article/link/assets/decisions/revision/outbox
→ 失败资源创建 retry_article_assets
→ 成功后推进 heartbeat（事件幂等）
```

发布时间优先使用详情可信值，其次使用候选发布时间；两者都没有则保持 `None`，不能使用抓取时间伪造。

### 6.4 历史回填

`history_backfill` 每个 Job 只读取一页。payload 保存 cursor、page_size、remaining_pages 和 run_id；成功处理当前页后才以幂等键创建下一页。执行前和创建下一页前都检查取消标记。遇到已有强身份边界可以提前停止。

### 6.5 文章变更与下游分发

新增、重抓、人工编辑、OCR 或资源变化如果影响下游可见结果，必须在同一业务提交中产生：

1. 当前文章状态/内容。
2. 不可变 revision。
3. `article_change_events`。
4. `delivery_outbox`。

下游通过分组 Token、scope 和单调游标消费，不直接依赖数据库更新时间猜测变化。

### 6.6 每日健康和计划

```text
07:30 daily_preflight
→ 检查 connector、出口、队列、对象存储等
→ 07:45 前保存当天不可变 health snapshot
08:00 build_daily_plan
→ 按 snapshot、来源状态、心跳和容量筛选
→ 工作窗口内非等间隔分配
→ 创建幂等 fetch_source Job
```

任务开始时仍需二次检查来源状态，但不能反复执行全量 preflight。

## 7. 身份、幂等与并发规则

- 文章身份优先：`source_account_id + __biz + mid + idx`。
- 文章身份后备：`source_account_id + canonical_url`。
- 内容 hash 用于变化检测和资产去重，不是全局文章身份。
- 写请求必须通过业务唯一键或 Job 幂等键（`job_pipeline::kinds::idempotency`）去重。`crates/server-core` 中的 `Idempotency-Key` 中间件尚未挂到任何路由，不能依赖该请求头。
- Job worker 依赖持久 lease；失去 lease 的旧 worker 不能提交结果。
- Heartbeat 使用 event key 幂等；多个详情成功不得重复推进。
- 时间逻辑使用注入的 `Clock`，测试中不得依赖真实 sleep 推进业务时间。

## 8. 错误模型

采集执行边界使用 `CaptureFailure`：

```text
code
category
retryable
account_effect
source_effect
retry_after_ms
safe_context
```

任务层根据结构化字段决定 retry、needs_action 和 heartbeat，不得用 `String.contains` 猜测错误类别。必须区分凭据失效、风控、平台限制、限流、临时网络、内容撤回/无效、存储临时/永久、配置错误和取消。

外部错误消息必须脱敏；safe context 只允许稳定 ID、错误码、状态和计数。

## 9. 本地运行

### 9.1 最小内存模式

```powershell
Copy-Item .env.example .env
$env:SIGNAL_STORE = "memory"
$env:SIGNAL_EMBED_WORKER = "1"
cargo run -p signal-service --bin signal-service
```

Memory/Fake 只允许 development/test 环境。

### 9.2 PostgreSQL + 独立 Worker

```powershell
Copy-Item .env.example .env
./scripts/init-dev-resources.ps1
$env:SIGNAL_STORE = "postgres"
$env:SIGNAL_SECRET_STORE_PATH = "./data/secrets"   # postgres 模式必填，无默认值
cargo run -p signal-service --bin signal-service
cargo run -p signal-service --bin signal-worker
```

`SIGNAL_SECRET_STORE_PATH`、`REDIS_URL` 和 `S3_ENDPOINT` 在 postgres 模式下没有默认值，缺失时启动报 `config.*_required`。应用启动时运行 SQLx migrations。生产环境必须配置真实 SecretStore、对象存储和非 Fake Provider，并更换所有本地默认凭据。

### 9.3 前端

```powershell
cd web
pnpm install
pnpm dev
```

## 10. 测试策略与常用命令

优先执行受影响范围，避免每次全量构建：

```powershell
# Connector 协议和分页
cargo test -p platform-connectors protocol

# URL、正文、SSRF、图片和对象存储
cargo test -p crawler-capture

# 来源验证
cargo test -p signal-service --test source

# 文章采集、资产重试和历史回填
cargo test -p signal-service --test article_capture

# Job lease/retry/cancel
cargo test -p job-pipeline

# 服务纯逻辑
cargo test -p signal-service --lib

# 最终检查
cargo fmt --all -- --check
cargo clippy -p platform-connectors -p crawler-capture -p signal-service --tests -- -D warnings
```

不要默认执行 `cargo build --workspace`、`cargo test --workspace` 或真实平台端到端测试。前端发生变化时再运行：

```powershell
pnpm -C web build
```

## 11. 修改工作流

1. 确认产品边界和涉及的模块。
2. 查找现有 trait、service、repository 和 fake；优先扩展现有边界。
3. 先写定向失败测试或 fixture。
4. 实现最小修改，不顺带重构无关模块。
5. 同时维护 memory 与 postgres repository 行为。
6. 数据变更新增 migration，不修改旧 migration。
7. 外部能力必须注入、限时、可取消、可限流并有安全错误。
8. 运行受影响测试、`cargo check`、格式检查；阶段完成后再跑 Clippy。
9. 同步 README、本文或专题文档中受影响的事实。

## 12. 当前完成状态

截至 2026-07-17，`SIGNAL_REPAIR_LOOP_PLAN.md` 的采集修复已完成并通过本地验证：

- 候选发布时间、作者、封面和脱敏证据贯穿 Job。
- 旧候选不会被误报为新文章，heartbeat 在提交成功后推进。
- 微信 URL、短链、敏感参数和重定向边界已收紧。
- MP Admin 支持受限分页，历史回填按页 checkpoint。
- 正文保留语义结构，图片位置稳定。
- 单资源失败不丢正文，并有独立重试 Job。
- 任务边界使用结构化错误。
- list/detail/media/search 有共享多维请求预算。

最后一次记录的本地验收：`platform-connectors protocol` 9 项、`crawler-capture` 31 项、`source` 15 项、`article_capture` 14 项、`job-pipeline` 29 项、`signal-service --lib` 132 项，以及相关 Clippy `-D warnings` 通过。

## 13. 已知限制与后续方向

- 真实 OCR Provider 尚未接入；当前 Fake 仅供 development/test。
- `weread` 没有经过授权、协议冻结和验收，必须保持显式 unsupported。
- 平台 DOM/协议可能变化，新增回归应使用脱敏 fixture，不把真实响应或凭据提交进仓库。
- 生产 SecretStore、代理 Provider、对象存储和浏览器运行时需要部署级配置与单独验收。
- 项目以 MIT 许可证发布（见根目录 `LICENSE`，Cargo 元数据 `license = "MIT"`）；新增依赖时注意第三方许可兼容性。
- 当前系统设计为单管理员；若未来需要多人审计，应作为独立产品升级，不要在现有模型中偷偷增加角色。

## 14. 接手检查清单

开始开发前确认：

- [ ] 已理解唯一管理员和授权采集边界。
- [ ] 已确认任务属于 API、Service、Domain、Repository、Job 还是 Adapter。
- [ ] 已找到对应 fake/fixture 和最小测试命令。
- [ ] 不会把凭据、正文或代理端点写入日志/safe context。
- [ ] 不会绕过 SSRF、限流、SecretStore、Job lease 或 revision/outbox。
- [ ] 数据变更使用新 migration。
- [ ] 状态转换和时间逻辑可确定性测试。
- [ ] 修改后同步相关文档。
