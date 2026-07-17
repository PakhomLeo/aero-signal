/** 后端统一错误外壳。 */
export interface ApiErrorBody {
  code: string
  message: string
  details?: Record<string, unknown>
  trace_id?: string
}

export interface AdministratorPublic {
  username: string
  must_change_password: boolean
  password_changed_at: string | null
  created_at: string
  updated_at: string
}

export interface LoginResponse {
  token: string
  expires_at: string
  admin: AdministratorPublic
}

export interface JobResponse {
  id: string
  kind: string
  status: string
  progress_percent: number
  progress_message: string
  progress_stage: string
  attempt_count: number
  max_attempts: number
  idempotency_key: string | null
  next_run_at: string | null
  result_json: unknown | null
  last_error_code: string | null
  last_error_message: string | null
  action_code: string | null
  action_message: string | null
  safe_context_json: unknown | null
  created_at: string
  updated_at: string
}

export interface HealthComponentAdminDto {
  component_type: string
  component_id: string | null
  status: string
  reason_code: string | null
  checked_at: string
  safe_details: unknown
}

export interface HealthSnapshotAdminDto {
  schema_version: number
  id: string
  snapshot_date: string
  generated_at: string
  overall_status: string
  yesterday_plan_success_count: number
  yesterday_plan_failure_count: number
  yesterday_article_failure_count: number
  today_eligible_source_count: number
  today_blocked_source_count: number
  pending_action_count: number
  summary: unknown
  components: HealthComponentAdminDto[]
}

export interface DailyPlanRowDto {
  id: string
  plan_date: string
  source_account_id: string
  planned_run_at: string
  status: string
  reason_code: string
  fetch_job_id: string | null
  collector_account_id: string | null
}

export interface DailyPlanListDto {
  plan_date: string
  snapshot_id: string | null
  overall_status: string | null
  can_build_executable: boolean
  block_reason: string | null
  rows: DailyPlanRowDto[]
  eligible_count: number
  planned_count: number
  started_count: number
}

export interface CollectorAccountPublic {
  id: string
  connector: string
  display_name: string
  external_account_ref: string | null
  status: string
  fixed_egress_id: string | null
  requires_relogin: boolean
  last_probe_at: string | null
  last_success_at: string | null
  last_failure_at: string | null
  last_error_code: string | null
  created_at: string
  updated_at: string
}

/** 二维码会话：仅含 data URL，禁止第三方原始 URL。 */
export interface QrSessionResponse {
  ticket: string
  qr_data_url: string
  expires_at: string
  poll_after_ms: number
  status: string
  account?: CollectorAccountPublic
  message?: string
}

export interface SourceAccountPublic {
  id: string
  version: number
  display_name: string
  status: string
  preferred_connector: string | null
  strong_ids: Array<{
    id_type: string
    id_value: string
    confidence: string
  }>
  added_at: string
  last_fetch_at: string | null
  fetch_rules: {
    base_level: string
    enabled: boolean
    first_heartbeat_at: string
    next_heartbeat_at: string
    working_windows: Array<{ start: string; end: string }>
    connector_override: string | null
    interval_override_hours: number | null
    manual_next_heartbeat_at: string | null
    auto_promotion_enabled: boolean
    updated_at: string
  } | null
}

export interface PatchSourceRequest {
  expected_version: number
  display_name?: string
  status?: string
  preferred_connector?: string | null
  base_level?: string
  enabled?: boolean
  working_windows?: Array<{ start: string; end: string }>
  connector_override?: string | null
  interval_override_hours?: number | null
  manual_next_heartbeat_at?: string | null
  auto_promotion_enabled?: boolean
}

export interface ValidationCandidateResult {
  canonical_url: string
  title?: string | null
  identity_key?: string | null
  status: string
  article_id?: string | null
  error_code?: string | null
  job_id?: string | null
}

export interface SourceValidationRun {
  id: string
  input_url: string
  status: string
  candidate_count: number
  successful_count: number
  available_count: number
  missing_reason: string | null
  candidates: ValidationCandidateResult[]
  source_account_id: string | null
  expires_at: string
  created_at: string
  completed_at: string | null
  parsed_source_json?: unknown
}

export interface ValidateSourceResponse {
  run: SourceValidationRun
  existing_source: SourceAccountPublic | null
  idempotency_key: string
  already_exists: boolean
}

export interface FetchNowResponse {
  job_id: string
  idempotency_key: string
  recalculate_next: boolean
}

export interface Article {
  id: string
  source_account_id: string
  article_link_id: string | null
  original_url: string
  canonical_url: string
  original_title: string
  author: string | null
  published_at: string | null
  fetched_at: string
  current_revision: number
  content_md: string
  content_hash: string
  asset_manifest_hash: string | null
  status: string
  withdrawn_at: string | null
}

export interface ArticleRevision {
  id: string
  article_id: string
  revision: number
  editor_kind: string
  content_md: string
  content_hash: string
  change_reason: string
  created_at: string
}

export interface ArticleDetailDto {
  article: Article
  revisions: ArticleRevision[]
}

export interface WorkingWindow {
  start: string
  end: string
}

export interface ScheduleSettings {
  timezone: string
  preflight_time: string
  plan_start_time: string
  working_windows: WorkingWindow[]
  image_min_side_px: number
  image_min_area_px: number
  ocr_min_confidence: number
}

export interface EgressProfilePublic {
  id: string
  name: string
  kind: string
  provider_id: string | null
  has_endpoint_secret: boolean
  priority: number
  allowed_purposes: string[]
  status: string
  last_preflight_status: string | null
  last_preflight_at: string | null
  created_at: string
  updated_at: string
}

export interface ProxyProviderPublic {
  id: string
  name: string
  mode: ProxyProviderMode
  api_base_url: string | null
  has_auth_secret: boolean
  request_template_json: JsonValue
  response_mapping_json: JsonValue
  enabled: boolean
  created_at: string
  updated_at: string
}

/** Provider modes are persisted by the backend and must stay in sync with its enum. */
export type ProxyProviderMode = 'static_endpoint' | 'gateway' | 'vendor_api'

/** JSON value accepted by the backend's serde_json::Value fields. */
export type JsonValue =
  | string
  | number
  | boolean
  | null
  | JsonValue[]
  | { [key: string]: JsonValue }

export interface CreateProxyProviderRequest {
  name: string
  mode: ProxyProviderMode
  api_base_url?: string | null
  auth_secret?: string
  request_template_json?: JsonValue
  response_mapping_json?: JsonValue
  enabled?: boolean
}

export interface PatchProxyProviderRequest {
  name?: string
  mode?: ProxyProviderMode
  api_base_url?: string
  auth_secret?: string
  clear_auth_secret?: boolean
  request_template_json?: JsonValue
  response_mapping_json?: JsonValue
  enabled?: boolean
}

export interface DeliveryGroupPublic {
  id: string
  name: string
  description: string | null
  status: string
  source_account_ids: string[]
  created_at: string
  updated_at: string
}

export interface ApiTokenPublic {
  id: string
  delivery_group_id: string
  name: string
  token_prefix: string
  scopes: string[]
  ip_allowlist: string[]
  expires_at: string | null
  revoked_at: string | null
  last_used_at: string | null
  created_at: string
  status: string
}

export interface IssueTokenResponse {
  token: ApiTokenPublic
  plaintext: string
}

export interface JobEnqueueResponse {
  job_id: string
  kind: string
  idempotency_key?: string | null
}
