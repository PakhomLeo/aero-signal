# Signal 循环修复计划

## 1. 目标

本计划用于逐步完善 Signal 的公众号采集链路，重点修复以下问题：

1. 候选文章被错误判断为新文章。
2. 发布时间、作者、封面和候选证据在任务链路中丢失。
3. 来源 URL 缺少微信域名校验、短链解析和敏感参数清理。
4. MP Admin 只获取第一页、最多 10 条文章。
5. HTML 转 Markdown 丢失标题、列表、链接、表格和音视频语义。
6. 单张图片失败导致整篇文章失败。
7. 错误类型在任务层退化为字符串匹配。
8. 缺少账号、出口、主机和接口级的 HTTP 请求限流。

参考项目代码可以直接移植，但进入 Signal 后必须适配现有的 trait、持久 Job、SecretStore、强身份、revision/outbox、SSRF 防护和媒体安全模型。

本计划只面向本地开发和自动化 fixture/fake 测试，不要求每轮连接真实微信、代理、对象存储或 OCR 服务。

---

## 2. 固定执行循环

每轮只处理一个可验证的行为问题：

```text
选择一个未完成问题
→ 定位 Signal 代码和参考实现
→ 先补一个定向失败测试
→ 移植或实现最小修复
→ 运行对应测试
→ 检查受影响 crate
→ 审查 diff、边界和错误处理
→ 标记完成并进入下一轮
```

### 2.1 每轮限制

- 一轮只解决一个行为问题。
- 尽量控制在 1～4 个生产文件。
- 数据迁移、公共模型和业务行为跨度过大时必须拆轮。
- 不顺手重构无关代码。
- 不自动提交 Git。
- 不因局部修改执行全 workspace build/test。
- 不访问真实微信、真实代理、真实对象存储或真实 OCR。
- 不扩大到 PRD 之外的产品功能。
- 遇到现有基线失败时先记录；只有阻塞本轮时才处理。

### 2.2 每轮轻量检查

先定位和检查变更：

```powershell
rg -n "目标符号|错误码|字段名" .
git diff --stat
git diff --check
```

只运行受影响的测试：

```powershell
cargo test -p platform-connectors protocol
cargo test -p crawler-capture extractor
cargo test -p signal-service --test source
cargo test -p signal-service --test article_capture
```

只检查受影响的 crate：

```powershell
cargo check -p platform-connectors
cargo check -p crawler-capture
cargo check -p signal-service
```

格式检查：

```powershell
cargo fmt --all -- --check
```

执行原则：

- 修改 connector 时优先测试 `platform-connectors`。
- 修改抓取、HTML 或图片时优先测试 `crawler-capture`。
- 修改文章保存、任务或 heartbeat 时运行对应的 `signal-service` 集成测试。
- 只有公共类型跨 crate 变化时才检查依赖它的下游 crate。
- 没有修改前端时不运行 `pnpm build`。
- `cargo clippy` 放在最终验收，不在每轮执行。
- 默认不运行 `cargo build --workspace` 和 `cargo test --workspace`。

---

## 3. 阶段 0：冻结本地基线

### Loop 0.1：记录局部测试基线

执行：

```powershell
cargo test -p platform-connectors protocol
cargo test -p crawler-capture extractor
cargo test -p signal-service --test source
cargo test -p signal-service --test article_capture
```

要求：

- 记录通过项和已有失败项。
- 不启动数据库、Redis、MinIO 等重型服务，除非测试明确依赖本地集成环境。
- 不为了得到绿色基线而修复无关问题。

完成标准：获得后续循环可对照的本地测试基线。

---

## 4. 阶段 1：修复候选数据丢失

### Loop 1.1：传递发布时间

实现：

1. 给详情任务 payload 增加 `candidate_published_at`。
2. connector 候选时间传入详情任务。
3. 详情页提取到可信时间时优先使用详情值。
4. 详情没有时间时使用列表候选时间。
5. 两者都没有时保持 `None`，不得使用抓取时间伪装发布时间。

参考：

- 参考实现中的抓取流水线与 fetcher 服务（候选元数据透传方式）。

定向测试：

- 候选有时间、详情无时间。
- 候选与详情都有时间，详情优先。
- 两者都没有时间。
- Job 序列化和重试后时间不丢失。

命令：

```powershell
cargo test -p signal-service --test article_capture published
cargo check -p signal-service
```

### Loop 1.2：补充候选元数据

增加最小必要字段：

- `candidate_author`
- `candidate_cover_url`
- `candidate_published_at`
- 结构化、脱敏的 `candidate_evidence`

候选证据只允许保存：

- connector 类型。
- 页码或 cursor。
- 文章强身份。
- 标题、作者、时间、封面 URL。
- 响应摘要 hash。
- `observed_at`。

禁止保存：

- Cookie、token、二维码临时状态。
- `key`、`pass_ticket`、`appmsg_token`。
- 未脱敏完整响应。
- 代理端点或认证信息。

这一轮只打通模型和数据传递，不实现封面下载。

---

## 5. 阶段 2：修复新增文章判断

### Loop 2.1：候选去重

将当前“候选列表非空就是发现新文章”的逻辑替换为：

```text
候选列表
→ 规范强身份
→ 批内去重
→ 查询已有文章
→ 查询已有 pending/running 详情任务
→ 只入队真正的新候选
```

身份优先级：

```text
source + biz + mid + idx
→ canonical URL
```

内容 hash 不作为全局文章身份。

参考：

- 参考实现中的批内 URL set 和数据库过滤逻辑。
- wewe-wechat-archiver 的 URL 索引逻辑。
- 最终实现必须保留 Signal 的强身份和幂等 Job，不能退回仅 URL 去重。

测试：

- 同一批候选重复。
- 同一文章携带不同无关查询参数。
- 数据库已有文章。
- 已存在 pending/running 详情任务。
- 相同 URL 属于不同 source。
- 强身份相同但 URL 参数顺序不同。

### Loop 2.2：延迟提交 `found_new`

调整规则：

- discovery 只记录候选数量和新详情任务数量。
- 新文章保存成功或产生符合规则的新 revision 后，才提交 `found_new=true`。
- 所有候选均已存在时，记为“成功但无新文章”。
- 详情技术失败、登录失败、验证码或平台限制不能记为空抓。
- 重复任务不得重复推进 heartbeat。

测试：

- 10 个旧候选必须判定为空抓。
- 新候选详情失败不得记空抓。
- 新文章成功只推进一次。
- 详情重试不会重复推进。
- revision 的更新计数符合业务规则。

命令：

```powershell
cargo test -p signal-service --test article_capture heartbeat
cargo test -p signal-service --test article_capture duplicate
cargo check -p signal-service
```

---

## 6. 阶段 3：收紧来源 URL

### Loop 3.1：微信 URL 校验和敏感参数清理

可以直接移植 wewe-wechat-archiver 的处理规则：

- 微信 host allowlist。
- 微信文章路径验证。
- scheme 和 host 规范化。
- 移除 userinfo、fragment 和异常端口。
- 清除 `uin`、`key`、`pass_ticket`、`appmsg_token` 等敏感参数。
- 保留文章强身份需要的参数。
- 查询参数稳定排序。

参考：

- `reference-projects/wewe-wechat-archiver/archiver/url_utils.py`

实现时使用 Signal 现有 URL/SSRF 类型和稳定错误码。

测试：

- 合法公众号文章 URL。
- 非微信 HTTP URL。
- host 后缀欺骗和 userinfo 欺骗。
- 带敏感参数的文章 URL。
- URL fragment。
- 参数顺序不同但 canonical 相同。
- 编码后的危险 host/path。

### Loop 3.2：短链和重定向解析

流程：

```text
输入短链
→ SSRF 校验
→ 有限次数重定向
→ 每一跳重新校验
→ 验证最终微信 host/path
→ 提取 biz/mid/idx/sn
→ 必要时从可信 HTML 补充 biz
```

边界：

- 复用现有重定向逐跳防护。
- 限制跳转次数、响应大小和超时。
- 不执行浏览器自动化。
- 不处理验证码。
- 不接受私网、文件协议或非微信最终页面。

命令：

```powershell
cargo test -p signal-service --test source
cargo test -p crawler-capture identity
cargo check -p signal-service
```

---

## 7. 阶段 4：实现增量分页

### Loop 4.1：MP Admin 分页协议

参考常见实现的 `begin + count + can_msg_continue` 处理逻辑：

```text
读取 cursor
→ 请求一页
→ 解析主图文和多图文子项
→ 去重
→ 判断是否遇到已知边界
→ 根据 can_msg_continue 决定是否继续
→ 返回 next_cursor
```

Signal 必须增加明确预算：

- 正常增量抓取使用较小页数上限。
- 首次验证仍然最多抓取 3 篇不同文章。
- 历史回填使用独立 Job 和 checkpoint。
- 达到请求预算立即停止并保存 cursor。

仍然只允许处理已授权管理的公众号，不得把 `mp_admin` 改成任意第三方公众号枚举器。

测试：

- 单页和多页。
- `can_msg_continue=false`。
- 空页。
- 多图文子项。
- 跨页重复。
- 遇到已有强身份后提前停止。
- 达到页数或文章数预算。

命令：

```powershell
cargo test -p platform-connectors protocol
cargo check -p platform-connectors
cargo check -p signal-service
```

### Loop 4.2：历史回填 Job

范围只包括：

- cursor/checkpoint。
- 最大页数和文章数。
- stop requested。
- 取消后可恢复。
- 每页幂等。
- 已有文章停止或跳过策略。

借鉴参考实现的日期扫描和分页字段，但使用 Signal 的持久 Job，不复制内存任务实现。

---

## 8. 阶段 5：正文语义保真

### Loop 5.1：DOM 清洗

采用 allowlist 思路：

- 删除 script、style、广告和确定的非正文节点。
- 处理 `data-src`、`data-original` 等懒加载图片。
- 保留标题、段落、列表、引用、链接、表格、强调和代码。
- 链接协议只允许安全值。
- 不保留任意 CSS style。
- 音频和视频生成结构化占位。

保留以下产物：

- raw HTML 对象引用。
- sanitized rich HTML。
- semantic content blocks。
- clean text。
- Markdown。

### Loop 5.2：ContentBlock 转 Markdown

停止使用正则删除全部 HTML 标签，至少支持：

- Heading。
- Paragraph。
- OrderedList/UnorderedList。
- Blockquote。
- Link。
- CodeBlock。
- Table。
- Image。
- EmbeddedMedia。

图片必须以稳定锚点嵌入，不能因匹配失败全部追加到文章末尾。

参考：

- 参考实现中的富文本白名单。
- wewe-wechat-archiver 的 markdownify 和代码块占位。
- Signal 现有 OCR anchor、图片决策和 ContentBlock 必须继续保留。

测试使用本地 HTML fixture，覆盖：

- 标题、列表和链接。
- 代码块和表格。
- 段落内图片和连续图片。
- lazy image。
- 音频和视频占位。
- 广告节点。
- 恶意链接和 style。

命令：

```powershell
cargo test -p crawler-capture extractor
cargo test -p signal-service --test article_capture markdown
cargo check -p crawler-capture
cargo check -p signal-service
```

---

## 9. 阶段 6：资源部分成功

### Loop 6.1：单资源失败不阻断正文

资源结果至少区分：

```text
kept
filtered
download_failed_retryable
download_failed_terminal
stored
```

行为：

- 正文成功后允许先保存。
- 失败图片保留原 URL。
- 记录结构化失败原因。
- 文章可以标记为 `ready_with_asset_warning`。
- 创建独立资源重试任务。
- 重试成功后新增 revision/change event/outbox。
- 资源失败不能静默吞掉。

参考项目只提供“单图失败后继续保存”的行为参考；不得照搬参考实现中静默返回 `None` 的错误处理。

测试：

- 一张成功、一张超时。
- 一张被过滤。
- 对象存储暂态失败。
- 资源重试成功。
- 重试任务重复执行。
- 资源变化触发 revision。
- 正文仍然可以读取。

### Loop 6.2：封面图

在资源部分成功稳定后实现：

- 封面和正文图片分开建模。
- 使用同一套资产下载安全规则。
- 封面失败不影响正文。
- 内容相同的封面和正文图通过 hash 复用。

---

## 10. 阶段 7：结构化错误和限流

### Loop 7.1：结构化错误贯穿任务链路

禁止上层依赖 `String.contains` 分类错误。

统一错误至少包括：

- `code`
- `category`
- `retryable`
- `account_effect`
- `source_effect`
- `retry_after`
- 脱敏后的 safe context

分类至少覆盖：

- credentials invalid。
- risk control。
- platform restriction。
- rate limited。
- network temporary。
- content withdrawn。
- content invalid。
- storage temporary/permanent。
- configuration error。
- cancelled。

测试每类错误是否：

- 重试。
- 冷却账号。
- 记空抓。
- 切换出口。
- 通知管理员。

### Loop 7.2：HTTP 请求级多维限流

限流维度：

- collector account。
- egress lease/IP。
- host。
- endpoint purpose。
- monitored source。

覆盖请求：

- list。
- detail。
- media。
- search/profile。

只实现正常请求预算、最小间隔和服务端 `retry_after`，不实现随机规避、设备伪装或平台限制绕过。

命令：

```powershell
cargo test -p platform-connectors rate
cargo test -p signal-service connector_context
cargo test -p signal-service --test article_capture retry
cargo check -p signal-service
```

---

## 11. 不纳入本计划的内容

为了避免超出产品和安全边界，本计划不实现：

- 任意第三方公众号枚举。
- WAF、验证码或风控绕过。
- Frida、MITM 或内存凭证扫描。
- 鼠标模拟和设备指纹轮换。
- 未经授权自动切换出口继续请求。
- 未验收的 WeRead 非公开协议。
- AI 摘要、推荐和内容生成。
- 评论、阅读量、点赞和分享统计。
- PDF、DOCX、MHTML 等多格式导出。
- 大规模前端重做。
- 微服务拆分或数据库体系重建。
- 每轮真实平台端到端测试。

wechatDownload 的暂停、日期选择、跳过已下载等体验只作为后续管理端候选，不进入本轮核心正确性修复。

---

## 12. 阶段性审查

每完成一个阶段，只执行一次组合检查：

```powershell
cargo fmt --all -- --check
cargo check -p platform-connectors
cargo check -p crawler-capture
cargo check -p signal-service
git diff --check
```

按改动选择相关测试，不默认全部运行：

```powershell
cargo test -p platform-connectors
cargo test -p crawler-capture
cargo test -p signal-service --test source
cargo test -p signal-service --test article_capture
```

全部阶段完成后才执行一次：

```powershell
cargo clippy -p platform-connectors -p crawler-capture -p signal-service --tests -- -D warnings
cargo test -p signal-service --lib
```

仍然不默认执行：

```powershell
cargo build --workspace
cargo test --workspace
pnpm -C web build
```

只有跨 workspace 公共接口发生变化或前端被修改时，才增加相应检查。

---

## 13. 完成标准

满足以下条件后结束循环：

- 重复旧候选不再被报告为新文章。
- 发布时间不会在任务链路中丢失。
- 敏感微信查询参数不会进入 canonical URL、日志和数据库。
- MP Admin 支持受限、可恢复的增量分页。
- HTML 的标题、列表、链接、代码、表格和图片位置得到保留。
- 单张图片失败不会导致整篇正文丢失。
- 错误分类不再依赖字符串匹配。
- 空抓、技术失败、凭证失效和平台限制具有不同状态转换。
- 所有新增逻辑都有 fake/fixture 定向测试，不依赖真实外部服务。
- SecretStore、Job lease、revision/outbox、SSRF 和媒体安全能力没有退化。

推荐执行顺序：

```text
发布时间
→ 新增判定
→ URL 边界
→ 增量分页
→ 正文解析
→ 资源降级
→ 结构化错误
→ HTTP 限流
```

---

## 14. 执行结果（2026-07-17）

本计划已按本地 fake/fixture 循环完成，未连接真实微信、代理、对象存储或 OCR 服务，也未执行 workspace 全量构建/测试。

### 14.1 已完成项

- 候选数据：详情任务完整携带发布时间、作者、封面和脱敏证据；详情值优先，候选值作为回退，不使用抓取时间伪造发布时间。
- 新增判断：按 `source + biz + mid + idx`、canonical URL 和持久 Job 幂等键去重；只有文章成功提交后才推进 `found_new` 心跳，旧候选批次记录为空抓成功，技术/平台失败不记为空抓。
- URL 边界：限定微信文章 host/path，规范化 scheme/host/端口，移除 userinfo、fragment 和敏感查询参数并稳定排序。
- 短链：仅接受受支持的腾讯短链入口；使用现有 HTTP 抓取器逐跳进行 SSRF 校验，最多 5 次重定向，并沿用超时和 HTML 大小限制；最终页必须是微信文章，可从可信最终 HTML 补充缺失的 `biz`。测试使用内存抓取器，不访问外网。
- 增量分页：MP Admin 支持 `begin/count/can_msg_continue`、跨页去重和页数预算；初始验证仍最多取 3 篇不同文章。
- 历史回填：增加独立持久 Job，每个 Job 处理一页并把 cursor 作为 checkpoint；支持预算、已知强身份边界、取消检查、下一页幂等创建和恢复执行。
- 正文语义：DOM allowlist 清洗保留标题、段落、列表、引用、链接、代码、表格、图片位置和音视频占位，同时产出 raw HTML、sanitized HTML、clean text 和 Markdown。
- 资源降级：正文图片或封面单项下载/存储失败不丢正文，持久化结构化失败决策并创建独立资源重试 Job；重试成功可产生新 revision/change event/outbox。
- 封面：封面与正文图片分别建模并复用同一下载安全规则；封面失败不阻断正文。
- 结构化失败：采集任务边界使用 `CaptureFailure`，包含 code、category、retryable、account/source effect、retry_after 和 safe context；任务处理器不再通过 `String.contains` 分类。
- 多维限流：共享 token bucket 同时约束 collector、egress、source、host 和 endpoint purpose；覆盖 list、detail、media 及短链 search 请求，服务端 retry-after 可进入结构化失败；未实现随机规避、设备伪装或平台限制绕过。

### 14.2 本地验收记录

以下检查已通过：

```text
cargo test -p platform-connectors protocol                 9 passed
cargo test -p crawler-capture                              31 passed
cargo test -p signal-service --test source                 15 passed
cargo test -p signal-service --test article_capture        14 passed
cargo test -p job-pipeline                                 29 passed
cargo test -p signal-service --lib                        132 passed
cargo check -p platform-connectors                          passed
cargo check -p crawler-capture                              passed
cargo check -p signal-service                               passed
cargo clippy -p platform-connectors -p crawler-capture \
  -p signal-service --tests -- -D warnings                  passed
cargo fmt --all -- --check                                  passed
git diff --check                                            passed
```

Windows MSVC 链接器仅输出“正在创建库”的提示，没有测试或 Clippy 告警。
