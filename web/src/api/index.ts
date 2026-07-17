import { apiRequest } from './client'
import type {
  AdministratorPublic,
  ApiTokenPublic,
  ArticleDetailDto,
  CollectorAccountPublic,
  CreateProxyProviderRequest,
  DailyPlanListDto,
  DeliveryGroupPublic,
  EgressProfilePublic,
  FetchNowResponse,
  HealthSnapshotAdminDto,
  IssueTokenResponse,
  JobEnqueueResponse,
  JobResponse,
  LoginResponse,
  ProxyProviderPublic,
  PatchProxyProviderRequest,
  PatchSourceRequest,
  QrSessionResponse,
  ScheduleSettings,
  SourceAccountPublic,
  ValidateSourceResponse,
  SourceValidationRun,
} from './types'

export const authApi = {
  login(username: string, password: string) {
    return apiRequest<LoginResponse>('/auth/login', {
      method: 'POST',
      auth: false,
      body: { username, password },
    })
  },
  logout() {
    return apiRequest<{ ok: boolean }>('/auth/logout', { method: 'POST' })
  },
  getMe() {
    return apiRequest<AdministratorPublic>('/me')
  },
  patchMe(body: {
    username?: string
    current_password?: string
    new_password?: string
  }) {
    return apiRequest<AdministratorPublic>('/me', { method: 'PATCH', body })
  },
}

export const overviewApi = {
  latestSnapshot() {
    return apiRequest<HealthSnapshotAdminDto>('/health-snapshots/latest')
  },
  checkNow(body: { target_type?: string; target_id?: string } = {}) {
    return apiRequest<JobEnqueueResponse & { snapshot_overwrite: boolean }>(
      '/health-snapshots/check-now',
      { method: 'POST', body: { target_type: 'all', ...body } },
    )
  },
  /** 入队日预检（写不可变健康快照）；与 checkNow 不同。 */
  runPreflight() {
    return apiRequest<JobEnqueueResponse & { snapshot_date: string }>(
      '/health-snapshots/run-preflight',
      { method: 'POST', body: {} },
    )
  },
  dailyPlan(date?: string) {
    const q = date ? `?date=${encodeURIComponent(date)}` : ''
    return apiRequest<DailyPlanListDto>(`/daily-plan${q}`)
  },
  rebuildPlan(date?: string) {
    return apiRequest<JobEnqueueResponse & { plan_date: string; rebuild: boolean }>(
      '/daily-plan/rebuild',
      { method: 'POST', body: { date: date || null } },
    )
  },
}

export const collectorApi = {
  list() {
    return apiRequest<CollectorAccountPublic[]>('/collector-accounts')
  },
  patch(id: string, body: Record<string, unknown>) {
    return apiRequest<CollectorAccountPublic>(`/collector-accounts/${id}`, {
      method: 'PATCH',
      body,
    })
  },
  createQr(body: { connector: string; fixed_egress_id: string }) {
    return apiRequest<QrSessionResponse>('/collector-accounts/qr-sessions', {
      method: 'POST',
      body,
    })
  },
  pollQr(ticket: string) {
    return apiRequest<QrSessionResponse>(
      `/collector-accounts/qr-sessions/${encodeURIComponent(ticket)}`,
    )
  },
  cancelQr(ticket: string) {
    return apiRequest<QrSessionResponse>(
      `/collector-accounts/qr-sessions/${encodeURIComponent(ticket)}`,
      { method: 'DELETE' },
    )
  },
  /** Fake/dev：推进二维码状态（scanned | confirmed） */
  advanceQr(ticket: string, status: 'scanned' | 'confirmed') {
    return apiRequest<QrSessionResponse>(
      `/collector-accounts/qr-sessions/${encodeURIComponent(ticket)}/advance`,
      { method: 'POST', body: { status } },
    )
  },
  probeNow(id: string) {
    return apiRequest<CollectorAccountPublic>(`/collector-accounts/${id}/probe-now`, {
      method: 'POST',
      body: {},
    })
  },
  reauthorize(id: string, body: { fixed_egress_id?: string } = {}) {
    return apiRequest<CollectorAccountPublic>(`/collector-accounts/${id}/reauthorize`, {
      method: 'POST',
      body,
    })
  },
}

export const sourcesApi = {
  list() {
    return apiRequest<SourceAccountPublic[]>('/sources')
  },
  get(id: string) {
    return apiRequest<SourceAccountPublic>(`/sources/${id}`)
  },
  validate(body: {
    url: string
    collector_account_id?: string
  }) {
    return apiRequest<ValidateSourceResponse>('/sources/validate', {
      method: 'POST',
      body,
    })
  },
  getValidation(id: string) {
    return apiRequest<SourceValidationRun>(`/source-validations/${id}`)
  },
  confirm(body: {
    validation_run_id: string
    display_name?: string
    base_level: string
    working_windows: Array<{ start: string; end: string }>
    first_heartbeat_at: string
    preferred_connector?: string
  }) {
    return apiRequest<SourceAccountPublic>('/sources', { method: 'POST', body })
  },
  patch(id: string, body: PatchSourceRequest) {
    return apiRequest<SourceAccountPublic>(`/sources/${id}`, { method: 'PATCH', body })
  },
  verifyAgain(id: string, url?: string) {
    return apiRequest<ValidateSourceResponse>(`/sources/${id}/verify-again`, {
      method: 'POST',
      body: { url: url || null },
    })
  },
  fetchNow(id: string, recalculate_next = false, request_id = crypto.randomUUID()) {
    return apiRequest<FetchNowResponse>(`/sources/${id}/fetch-now`, {
      method: 'POST',
      body: { recalculate_next, request_id },
    })
  },
}

export const articlesApi = {
  get(id: string) {
    return apiRequest<ArticleDetailDto>(`/articles/${id}`)
  },
  patchContent(id: string, content_md: string, change_reason?: string, expected_revision?: number) {
    return apiRequest<ArticleDetailDto>(`/articles/${id}/content`, {
      method: 'PATCH',
      body: { content_md, change_reason, expected_revision },
    })
  },
  reocr(id: string, request_id = crypto.randomUUID()) {
    return apiRequest<{ job_ids: string[] }>(`/articles/${id}/reocr`, {
      method: 'POST',
      body: { request_id },
    })
  },
  refetch(id: string, request_id = crypto.randomUUID()) {
    return apiRequest<{ job_id: string; idempotency_key: string }>(`/articles/${id}/refetch`, {
      method: 'POST',
      body: { request_id },
    })
  },
  restore(id: string, revision: number) {
    return apiRequest<ArticleDetailDto>(`/articles/${id}/revisions/${revision}/restore`, {
      method: 'POST',
      body: {},
    })
  },
}

export const deliveryApi = {
  listGroups() {
    return apiRequest<DeliveryGroupPublic[]>('/delivery-groups')
  },
  createGroup(body: {
    name: string
    description?: string
    source_account_ids?: string[]
  }) {
    return apiRequest<DeliveryGroupPublic>('/delivery-groups', { method: 'POST', body })
  },
  getGroup(id: string) {
    return apiRequest<DeliveryGroupPublic>(`/delivery-groups/${id}`)
  },
  patchGroup(id: string, body: Record<string, unknown>) {
    return apiRequest<DeliveryGroupPublic>(`/delivery-groups/${id}`, {
      method: 'PATCH',
      body,
    })
  },
  listTokens(groupId: string) {
    return apiRequest<ApiTokenPublic[]>(`/delivery-groups/${groupId}/tokens`)
  },
  issueToken(
    groupId: string,
    body: {
      name: string
      scopes: string[]
      ip_allowlist?: string[]
      expires_at?: string | null
    },
  ) {
    return apiRequest<IssueTokenResponse>(`/delivery-groups/${groupId}/tokens`, {
      method: 'POST',
      body,
    })
  },
  revokeToken(groupId: string, tokenId: string) {
    return apiRequest<ApiTokenPublic>(`/delivery-groups/${groupId}/tokens/${tokenId}`, {
      method: 'DELETE',
    })
  },
}

export const settingsApi = {
  getSchedule() {
    return apiRequest<ScheduleSettings>('/settings/schedule')
  },
  patchSchedule(body: ScheduleSettings) {
    return apiRequest<ScheduleSettings>('/settings/schedule', { method: 'PATCH', body })
  },
  listEgress() {
    return apiRequest<EgressProfilePublic[]>('/settings/egress-profiles')
  },
  createEgress(body: Record<string, unknown>) {
    return apiRequest<EgressProfilePublic>('/settings/egress-profiles', {
      method: 'POST',
      body,
    })
  },
  patchEgress(id: string, body: Record<string, unknown>) {
    return apiRequest<EgressProfilePublic>(`/settings/egress-profiles/${id}`, {
      method: 'PATCH',
      body,
    })
  },
  testEgress(id: string, purpose?: string) {
    return apiRequest<{ ok: boolean; message?: string; [k: string]: unknown }>(
      `/settings/egress-profiles/${id}/test`,
      { method: 'POST', body: purpose ? { purpose } : {} },
    )
  },
  listProviders() {
    return apiRequest<ProxyProviderPublic[]>('/settings/proxy-providers')
  },
  createProvider(body: CreateProxyProviderRequest) {
    return apiRequest<ProxyProviderPublic>('/settings/proxy-providers', {
      method: 'POST',
      body,
    })
  },
  patchProvider(id: string, body: PatchProxyProviderRequest) {
    return apiRequest<ProxyProviderPublic>(`/settings/proxy-providers/${id}`, {
      method: 'PATCH',
      body,
    })
  },
}

export const jobsApi = {
  get(id: string) {
    return apiRequest<JobResponse>(`/jobs/${id}`)
  },
  cancel(id: string) {
    return apiRequest<{ id: string; status: string; cancelled: boolean }>(
      `/jobs/${id}/cancel`,
      { method: 'POST' },
    )
  },
}
