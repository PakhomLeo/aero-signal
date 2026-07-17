/// <reference types="vite/client" />

interface ImportMetaEnv {
  /** 可选绝对 baseURL；缺省走相对路径 `/api/admin`（dev 由 Vite 代理）。 */
  readonly VITE_API_BASE_URL?: string
}

interface ImportMeta {
  readonly env: ImportMetaEnv
}
