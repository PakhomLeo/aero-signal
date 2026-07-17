# aero-signal 数据库详细设计

本文是 Aero Signal 的数据模型与迁移基线。

## 1. 设计原则

- UUID 主键、`timestamptz`、软停用替代日常物理删除。
- PostgreSQL 只保存业务事实和加密密钥引用；Cookie、二维码临时状态、token 明文、代理密码、完整 Authorization header 不入库。
- `system_administrator` 与 `system_settings` 必须各只有一条逻辑记录。
- 文章 identity 优先于内容 hash；所有影响下游的文章改变都写 revision 和 change event。

## 2. 表清单

| 表 | 用途 |
| --- | --- |
| `system_administrator` | 唯一后台管理员与密码 hash |
| `admin_sessions` | 管理员会话与撤销 |
| `egress_profiles` / `proxy_providers` | 本机、固定、动态出口和供应商配置引用 |
| `collector_accounts` / `collector_login_sessions` / `collector_egress_bindings` | 扫码后的采集账号、登录态引用与固定出口绑定 |
| `source_accounts` / `source_account_ids` | 监测公众号与强弱标识 |
| `source_validation_runs` | 添加时前三篇验证结果 |
| `account_fetch_rules` / `source_heartbeat_events` | 等级、首次锚点、下次心跳、延期/降级解释 |
| `service_health_snapshots` / `service_health_components` | 07:30 preflight 产生的每日不可变快照 |
| `daily_fetch_plans` / `fetch_jobs` / `fetch_attempts` | 08:00 计划与异步任务 |
| `article_links` / `articles` / `article_revisions` / `article_change_events` | 候选链接、文章和下游变更 |
| `article_assets` / `article_image_decisions` / `article_ocr_blocks` | 正文图片、筛选理由和 OCR 位置 |
| `delivery_outbox` | 文章变更、撤回和资产变化的可重放投递队列 |
| `delivery_groups` / `delivery_group_sources` / `api_tokens` / `api_call_logs` | 分组与内部 API |
| `audit_logs` / `system_settings` / `retention_policies` | 审计、唯一系统配置和数据保留策略 |

## 3. 唯一管理员

```sql
create table system_administrator (
  singleton boolean primary key default true check (singleton),
  username text not null unique,
  password_hash text not null,
  must_change_password boolean not null default true,
  password_changed_at timestamptz,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);

create table admin_sessions (
  id uuid primary key,
  session_hash text not null unique,
  expires_at timestamptz not null,
  revoked_at timestamptz,
  ip_hash text,
  user_agent_hash text,
  created_at timestamptz not null default now()
);
```

数据库 migration 只创建管理员容器，不填明文默认密码；首次部署由 secrets/环境变量执行受审计的 bootstrap。后台不存在 `roles`、`permissions` 或第二个用户表。

## 4. 出口、供应商与采集登录账号

```sql
create table proxy_providers (
  id uuid primary key,
  name text not null unique,
  mode text not null check (mode in ('static_endpoint','gateway','vendor_api')),
  api_base_url text,
  auth_secret_ref text,
  request_template_json jsonb not null default '{}',
  response_mapping_json jsonb not null default '{}',
  enabled boolean not null default true,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);

create table egress_profiles (
  id uuid primary key,
  name text not null unique,
  kind text not null check (kind in ('local_direct','fixed','dynamic')),
  provider_id uuid references proxy_providers(id) on delete restrict,
  endpoint_secret_ref text,
  priority int not null default 100,
  allowed_purposes text[] not null,
  status text not null default 'active',
  last_preflight_status text,
  last_preflight_at timestamptz,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);

create table collector_accounts (
  id uuid primary key,
  connector text not null check (connector in ('mp_admin','weread')),
  display_name text not null,
  external_account_ref text,
  status text not null check (status in ('active','needs_relogin','expired','disabled','failed')),
  capability_json jsonb not null default '{}',
  last_probe_at timestamptz,
  last_success_at timestamptz,
  last_failure_at timestamptz,
  last_error_code text,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);

create table collector_login_sessions (
  id uuid primary key,
  collector_account_id uuid not null references collector_accounts(id) on delete cascade,
  credential_secret_ref text not null,
  browser_profile_ref text,
  expires_at timestamptz,
  confirmed_at timestamptz not null,
  revoked_at timestamptz,
  unique (collector_account_id, revoked_at)
);

create table collector_egress_bindings (
  collector_account_id uuid primary key references collector_accounts(id) on delete cascade,
  fixed_egress_id uuid references egress_profiles(id) on delete restrict,
  bound_at timestamptz not null default now(),
  changed_at timestamptz,
  requires_relogin boolean not null default false
);
```

`allowed_purposes` 只能是 `login`、`discover`、`article_detail`、`asset_download`。固定出口仅允许前两项；动态出口仅允许后两项。`endpoint_secret_ref` 是包含地址/认证的密钥引用，不能拆成数据库明文列。

## 5. 监测公众号、首次验证与心跳

```sql
create table source_accounts (
  id uuid primary key,
  display_name text not null,
  status text not null check (status in ('verifying','pending_configuration','active','paused','needs_action','archived')),
  preferred_connector text,
  added_at timestamptz not null default now(),
  last_fetch_at timestamptz,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);

create table source_account_ids (
  id uuid primary key,
  source_account_id uuid not null references source_accounts(id) on delete cascade,
  id_type text not null check (id_type in ('biz','fakeid','alias','weread_mp_id')),
  id_value text not null,
  confidence text not null check (confidence in ('strong','weak')),
  unique (id_type, id_value)
);

create table source_validation_runs (
  id uuid primary key,
  input_url text not null,
  input_hash text not null unique,
  parsed_source_json jsonb not null,
  connector_account_id uuid references collector_accounts(id) on delete set null,
  status text not null check (status in ('running','succeeded','failed','expired')),
  candidate_count smallint not null default 0 check (candidate_count between 0 and 3),
  successful_count smallint not null default 0 check (successful_count between 0 and 3),
  expires_at timestamptz not null,
  created_at timestamptz not null default now(),
  completed_at timestamptz
);

create table account_fetch_rules (
  source_account_id uuid primary key references source_accounts(id) on delete cascade,
  base_level text not null check (base_level in ('LO0','LO1','L1','L2','L3','L4')),
  connector_override text,
  enabled boolean not null default true,
  first_heartbeat_at timestamptz not null,
  next_heartbeat_at timestamptz not null,
  working_windows jsonb not null,
  interval_override_hours int,
  consecutive_empty_fetches int not null default 0,
  promotion_until timestamptz,
  manual_next_heartbeat_at timestamptz,
  updated_at timestamptz not null default now()
);

create table source_heartbeat_events (
  id uuid primary key,
  source_account_id uuid not null references source_accounts(id) on delete cascade,
  outcome text not null check (outcome in ('new_article','empty_success','technical_failure','access_blocked','manual')),
  level_before text not null,
  level_after text not null,
  empty_count_after int not null,
  previous_next_at timestamptz,
  next_heartbeat_at timestamptz,
  reason_code text not null,
  created_at timestamptz not null default now()
);
create index idx_source_due on account_fetch_rules(next_heartbeat_at) where enabled;
```

强标识 `biz` 冲突必须拒绝创建。首次验证的三篇文章直接关联后续 source；验证失败的临时数据应有保留期清理任务。

## 6. 每日健康快照、计划与任务

```sql
create table service_health_snapshots (
  id uuid primary key,
  snapshot_date date not null unique,
  generated_at timestamptz not null,
  overall_status text not null check (overall_status in ('healthy','degraded','unhealthy')),
  yesterday_plan_success_count int not null default 0,
  yesterday_plan_failure_count int not null default 0,
  yesterday_article_failure_count int not null default 0,
  today_eligible_source_count int not null default 0,
  today_blocked_source_count int not null default 0,
  pending_action_count int not null default 0,
  summary_json jsonb not null default '{}'
);

create table service_health_components (
  id uuid primary key,
  snapshot_id uuid not null references service_health_snapshots(id) on delete cascade,
  component_type text not null check (component_type in ('collector','connector','fixed_egress','dynamic_egress','worker','object_store')),
  component_id uuid,
  status text not null check (status in ('healthy','degraded','unhealthy','not_configured')),
  reason_code text,
  checked_at timestamptz not null,
  safe_details_json jsonb not null default '{}'
);

create table daily_fetch_plans (
  id uuid primary key,
  plan_date date not null,
  source_account_id uuid not null references source_accounts(id) on delete restrict,
  health_snapshot_id uuid not null references service_health_snapshots(id) on delete restrict,
  planned_run_at timestamptz not null,
  status text not null default 'planned',
  reason_code text not null,
  heartbeat_event_id uuid references source_heartbeat_events(id) on delete set null,
  unique (plan_date, source_account_id)
);
create index idx_daily_plan_ready on daily_fetch_plans(status, planned_run_at);
```

`service_health_snapshots` 成功写入后不得更新；管理员单项检测放入 `fetch_attempts`，而非覆盖当天快照。

`fetch_jobs`、`fetch_attempts` 采用 `job-pipeline` 标准字段，并增加 `safe_context_json`；它只允许 job kind、HTTP 状态、对象数量、出口类型和错误码。

## 7. 文章、图片、OCR 与版本

```sql
create table article_links (
  id uuid primary key,
  source_account_id uuid not null references source_accounts(id) on delete cascade,
  canonical_url text not null,
  wechat_biz text,
  wechat_mid text,
  wechat_idx text,
  wechat_sn text,
  title_hint text,
  discovered_via text not null,
  discovered_at timestamptz not null default now(),
  unique (source_account_id, wechat_biz, wechat_mid, wechat_idx),
  unique (source_account_id, canonical_url)
);

create table articles (
  id uuid primary key,
  source_account_id uuid not null references source_accounts(id) on delete restrict,
  article_link_id uuid references article_links(id) on delete set null,
  original_url text not null,
  canonical_url text not null,
  original_title text not null,
  author text,
  published_at timestamptz,
  fetched_at timestamptz not null,
  current_revision int not null default 1,
  content_md text not null,
  content_hash text not null,
  asset_manifest_hash text,
  status text not null default 'ready',
  withdrawn_at timestamptz,
  unique (source_account_id, canonical_url)
);

create table article_revisions (
  id uuid primary key,
  article_id uuid not null references articles(id) on delete cascade,
  revision int not null,
  editor_kind text not null check (editor_kind in ('crawler','ocr','administrator')),
  content_md text not null,
  content_hash text not null,
  change_reason text not null,
  created_at timestamptz not null default now(),
  unique (article_id, revision)
);

create table article_assets (
  id uuid primary key,
  article_id uuid not null references articles(id) on delete cascade,
  original_url text,
  storage_key text not null,
  asset_hash text,
  width int,
  height int,
  byte_size bigint,
  mime_type text,
  asset_kind text not null check (asset_kind in ('body_image','raw_html','screenshot')),
  created_at timestamptz not null default now()
);

create table article_image_decisions (
  id uuid primary key,
  article_id uuid not null references articles(id) on delete cascade,
  candidate_url text not null,
  decision text not null check (decision in ('kept','decorative','too_small','hidden','duplicate','unsupported')),
  measured_width int,
  measured_height int,
  reason_json jsonb not null default '{}',
  asset_id uuid references article_assets(id) on delete set null,
  created_at timestamptz not null default now()
);

create table article_ocr_blocks (
  id uuid primary key,
  article_id uuid not null references articles(id) on delete cascade,
  asset_id uuid not null references article_assets(id) on delete cascade,
  block_anchor text not null,
  recognized_text text,
  confidence numeric(5,4),
  status text not null check (status in ('pending','succeeded','failed','manual_override')),
  provider_ref text,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now(),
  unique (article_id, asset_id, block_anchor)
);
```

`article_change_events` 使用单调 `change_sequence`、稳定 `event_id`、`article_revision`、`change_type` 和 `schema_version`；消费 API 以此 cursor 读取，浏览后台以 `(fetched_at,id)` 复合 cursor 读取。

## 8. 分组、Token 与审计

```sql
create table delivery_outbox (
  id uuid primary key,
  event_id uuid not null unique,
  change_sequence bigint not null,
  event_type text not null check (event_type in ('article_created','article_updated','article_refetched','article_edited','article_assets_changed','article_withdrawn')),
  article_id uuid not null references articles(id) on delete restrict,
  article_revision int not null,
  payload_schema_version int not null,
  payload_json jsonb not null,
  status text not null default 'pending' check (status in ('pending','delivering','delivered','failed','dead_letter')),
  attempt_count int not null default 0,
  next_attempt_at timestamptz,
  last_error_code text,
  created_at timestamptz not null default now(),
  delivered_at timestamptz
);
create index idx_delivery_outbox_pending on delivery_outbox(status, next_attempt_at, change_sequence);
create index idx_delivery_outbox_sequence on delivery_outbox(change_sequence);
```

`delivery_outbox` 与 `article_change_events` 在同一业务事务内写入。`/api/articles/since` 读取 `article_change_events` 作为事实 cursor；异步推送、重放和下游通知读取 `delivery_outbox`。outbox payload 只能包含对外 DTO 所需的稳定 ID、revision、change_type、schema_version 和资产引用，不保存 Cookie、代理、后台账号或原始错误正文。

`delivery_groups`、`delivery_group_sources`、`api_tokens`、`api_call_logs` 延续现有关系，但 `api_tokens.scopes` 增加 `service_health:read`。内部 token 必须绑定一个分组；健康 scope 不受文章来源过滤影响，但只返回脱敏快照。

`audit_logs` 的 `actor_type` 固定为 `system_administrator/api_token/system/worker`，不声称共享管理员账号能提供个人归因。敏感字段只记录变更类型、资源 ID、时间和结果。

```sql
create table retention_policies (
  id uuid primary key,
  target_kind text not null unique check (target_kind in ('raw_html','screenshot','body_image','ocr_text','api_call_log','audit_log','change_event','delivery_outbox','failed_validation_asset')),
  retention_days int not null check (retention_days > 0),
  hard_delete_after_days int check (hard_delete_after_days is null or hard_delete_after_days >= retention_days),
  enabled boolean not null default true,
  updated_at timestamptz not null default now()
);
```

清理任务必须先检查业务引用关系：仍被 `articles.current_revision`、`article_revisions`、`article_assets` 或未投递 outbox 引用的资产不得物理删除。合规撤回通过 `article_withdrawn` change event 与 tombstone payload 传播给下游，不能静默删除已消费事实。

## 9. 迁移顺序

## 10. 代码审核执行补充

- `source_heartbeat_events` 增加 `(source_account_id, event_key)` 唯一约束并由事务接口回放重复命令。
- `collector_login_sessions` 使用 `revoked_at IS NULL` partial unique index，保证每个 collector 只有一个 active session。
- 真实 PostgreSQL migration replay 需要配置 `DATABASE_URL` 并安装 `sqlx-cli`；本轮仅完成迁移文件与 Memory/PG 代码契约验证。

1. `system_administrator`、`admin_sessions`、`system_settings`
2. `proxy_providers`、`egress_profiles`
3. `collector_accounts`、`collector_login_sessions`、`collector_egress_bindings`
4. `source_accounts`、`source_account_ids`、`source_validation_runs`、`account_fetch_rules`、`source_heartbeat_events`
5. `service_health_snapshots`、`service_health_components`、`daily_fetch_plans`、job tables
6. article link/article/revision/asset/image decision/OCR/change event tables
7. `delivery_outbox`
8. delivery、token、API log、audit、retention policy tables
