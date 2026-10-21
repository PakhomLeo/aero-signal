# Signal 管理后台（web）

Aero Signal 管理端（M14 admin-web）。

技术栈：Vue 3 + TypeScript + Vite + Vue Router + Pinia。

## 本地启动

```powershell
cd web
pnpm install
pnpm dev
```

默认开发地址：`http://127.0.0.1:5173/`。开发服务器将 `/api` 代理到 `http://127.0.0.1:8080`；该目标写死在 `vite.config.ts` 中，需要改代理目标时请直接修改该文件（目前不读取任何环境变量）。

可选：构建或启动前设置 `VITE_API_BASE_URL` 为绝对地址（如 `http://127.0.0.1:8080`），前端会直接请求该地址而不走代理（见 `src/api/client.ts`）。注意后端目前未配置 CORS，跨源直连需要自行在反向代理层处理。

## 常用命令

| 命令 | 说明 |
| --- | --- |
| `pnpm install` | 安装依赖 |
| `pnpm dev` | 启动开发服务器 |
| `pnpm build` | 类型检查并生产构建 |
| `pnpm preview` | 预览构建产物 |

## 页面

| 路径 | 说明 |
| --- | --- |
| `/login` | 登录 |
| `/change-password` | 强制/主动改密 |
| `/overview` | 今日概览（快照 + 计划） |
| `/collector-accounts` | 采集登录账号 + 二维码弹窗 |
| `/sources` | 监测公众号列表 |
| `/sources/add` | 添加向导 |
| `/sources/:id` | 监测公众号详情 |
| `/articles/:id` | 文章详情（MD / 版本 / OCR / 重抓） |
| `/delivery` | 分组与 Token（明文仅一次） |
| `/settings` | 窗口 / 阈值 / 出口 / 供应商 |
| `/account` | 账户 |
| `/jobs/:id` | Job 进度 |

## 安全约定

- 会话 token 仅存 `sessionStorage`（管理员会话），不存采集 cookie / 采集 token。
- 二维码只展示服务端 `qr_data_url`（data URL），拒绝第三方原始 URL。
- 出口/供应商密钥只提交、不回显。
- 无注册 / 多用户 UI；共享账号限制文案在布局与登录页可见。

## 目录

- `src/api/` — 管理端 API 客户端
- `src/components/` — 共享账号提示、Job 进度、二维码弹窗
- `src/layouts/` — 管理壳
- `src/views/` — 业务页面
- `src/stores/` — Pinia（auth / app）
- `src/router/` — 路由与鉴权拦截
