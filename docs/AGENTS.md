# aero-signal AGENTS.md

开始开发前先阅读本目录的 PRD、技术方案、后端设计和数据库设计。

## 产品约束

- Signal 是单服务器后台；只有一个 `system_administrator`，没有注册、角色、会员、计费或普通用户前台。
- 监测公众号与采集登录账号必须分离建模、命名和 UI 展示。
- 初始管理员凭据仅由部署密钥 bootstrap；密码用 Argon2id，明文密码、Cookie、storage_state、二维码 ticket、token、代理认证信息均不得写日志或普通数据库字段。
- 采集登录账号只能由管理员二维码授权；二维码在前端以服务端生成的 data URL 展示，短期状态存 Redis，确认后只保存 secret/profile 引用。
- connector 必须声明并 probe capability；不得把未公开、未验收的平台网络请求视为稳定接口。`mp_admin` 不得被实现为任意第三方公众号枚举器。
- 不实现验证码/WAF 绕过、鼠标模拟、设备指纹轮换、未授权登录或因平台限制而自动换出口继续请求。

## 调度与内容规则

- 每日 07:30 做一次 preflight，07:45 前固化 health snapshot，08:00 后按该快照生成计划；任务运行中不得反复全量探测。
- 首次添加监测公众号最多验证抓取 3 篇不同文章；至少一篇成功才允许配置，首次抓取时间是后续心跳锚点。
- 只有“成功但无新文章”计入空抓和自动降级；技术错误、登录失效、验证码和平台限制不得计为空抓。
- LO0/LO1 不自动降级；L4 不进入自动计划。时间分配必须为容量约束下的非均匀随机抽样，不得等间隔排程。
- 文章 identity 优先使用 `source + __biz + mid + idx`，canonical URL 为后备；内容 hash 不是全局唯一键。
- 只保留正文图片；必须记录图片保留/丢弃原因。OCR 文本按图片位置回填 Markdown；人工编辑与重抓必须新增 revision。

## 工程实现规则

- 代码按 `api -> service -> domain -> repository -> adapter/job` 分层。API 层不跑抓取、OCR、preflight 或计划重建；超过 2 秒的操作必须创建 job。
- 状态转换只能由 domain transition method 完成；repository 和 SQL 不得直接拼状态字符串绕过业务规则。
- 所有写请求必须支持幂等键或业务唯一键；二维码、source 验证、每日计划、文章详情抓取和 OCR 重跑不得因重复点击产生重复事实。
- 所有影响下游可见结果的文章变更必须同时写 revision、`article_change_events` 和 `delivery_outbox`；不得只更新 `articles.content_md`。
- 所有外部能力通过 trait adapter 接入，并提供 fake adapter、超时、限流、失败和取消测试；单元测试不得依赖真实微信、代理、对象存储或 OCR。
- 时间相关逻辑必须通过 `Clock` trait 注入，覆盖二维码过期、Token 过期、07:30/08:00 调度、心跳延期/降级和保留期清理。
- 日志、审计、job safe context 和 metrics 只能写稳定 ID、错误码、状态和脱敏计数；不得写正文、Cookie、token、二维码原始 URL、代理端点或密钥。

## 验收重点

- QR 生成、扫码、确认、过期、取消、出口绑定改变后的重新授权。
- 三篇初始验证、首次锚点、心跳延期/降级、当天快照与 08:00 计划依赖。
- 固定出口仅登录/发现、动态出口仅详情/资产，网络故障与平台限制的处理差异。
- 懒加载图片、装饰图/小图过滤、OCR block、去重、编辑与 revision。
- `GET /api/service-health/latest` 脱敏、token scope 和不可变日快照。
