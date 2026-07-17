import type { ApiErrorBody } from './types'
import { labelForErrorCode } from './errorLabels'

const SESSION_KEY = 'signal_admin_session'

export class ApiError extends Error {
  readonly code: string
  readonly status: number
  readonly details: Record<string, unknown>
  readonly traceId: string

  constructor(status: number, body: ApiErrorBody | null, fallback: string) {
    const code = body?.code || `http.${status}`
    const message = body?.message || fallback
    super(message)
    this.name = 'ApiError'
    this.code = code
    this.status = status
    this.details = body?.details || {}
    this.traceId = body?.trace_id || ''
  }
}

export function getApiBaseUrl(): string {
  const raw = import.meta.env.VITE_API_BASE_URL?.trim()
  if (raw) return raw.replace(/\/$/, '')
  return ''
}

export function adminPath(path: string): string {
  const base = getApiBaseUrl()
  const p = path.startsWith('/') ? path : `/${path}`
  return `${base}/api/admin${p}`
}

export interface SessionSnapshot {
  token: string
  expiresAt: string
  username: string
  mustChangePassword: boolean
}

export function loadSession(): SessionSnapshot | null {
  try {
    const raw = sessionStorage.getItem(SESSION_KEY)
    if (!raw) return null
    return JSON.parse(raw) as SessionSnapshot
  } catch {
    return null
  }
}

export function saveSession(session: SessionSnapshot): void {
  sessionStorage.setItem(SESSION_KEY, JSON.stringify(session))
}

export function clearSession(): void {
  sessionStorage.removeItem(SESSION_KEY)
}

type HttpMethod = 'GET' | 'POST' | 'PATCH' | 'DELETE'

export interface RequestOptions {
  method?: HttpMethod
  body?: unknown
  token?: string | null
  /** 不带 Authorization（登录等）。 */
  auth?: boolean
  signal?: AbortSignal
  timeoutMs?: number
}

export async function apiRequest<T>(path: string, options: RequestOptions = {}): Promise<T> {
  const method = options.method || 'GET'
  const headers: Record<string, string> = {
    Accept: 'application/json',
  }
  if (options.body !== undefined) {
    headers['Content-Type'] = 'application/json'
  }

  const useAuth = options.auth !== false
  const token = options.token ?? (useAuth ? loadSession()?.token : null)
  if (useAuth && token) {
    headers.Authorization = `Bearer ${token}`
  }

  let response: Response
  const controller = new AbortController()
  const timeout = window.setTimeout(() => controller.abort(), options.timeoutMs ?? 15_000)
  const forwardAbort = () => controller.abort()
  options.signal?.addEventListener('abort', forwardAbort, { once: true })
  try {
    response = await fetch(adminPath(path), {
      method,
      headers,
      body: options.body === undefined ? undefined : JSON.stringify(options.body),
      signal: controller.signal,
    })
  } catch {
    window.clearTimeout(timeout)
    options.signal?.removeEventListener('abort', forwardAbort)
    throw new ApiError(0, null, '无法连接后端，请确认服务已启动或稍后重试')
  }

  window.clearTimeout(timeout)
  options.signal?.removeEventListener('abort', forwardAbort)

  if (response.status === 204) {
    return undefined as T
  }

  const text = await response.text()
  let json: unknown = null
  if (text) {
    try {
      json = JSON.parse(text)
    } catch {
      json = null
    }
  }

  if (!response.ok) {
    if (response.status === 401 && useAuth) {
      clearSession()
      window.location.replace('/login')
    }
    const errBody =
      json && typeof json === 'object' && 'error' in (json as object)
        ? ((json as { error: ApiErrorBody }).error)
        : null
    throw new ApiError(response.status, errBody, text || `请求失败 (${response.status})`)
  }

  return json as T
}

export function formatApiError(err: unknown): string {
  if (err instanceof ApiError) {
    const hint = labelForErrorCode(err.code)
    if (hint) {
      return err.traceId ? `${hint}（${err.code}）` : hint
    }
    if (err.code && err.code !== `http.${err.status}`) {
      return `${err.message} [${err.code}]`
    }
    return err.message
  }
  if (err instanceof Error) return err.message
  return String(err)
}
