# Signal 内部 crates

这些 crate 属于 Signal 项目并在本仓库内独立维护，不与其他 Aero 产品同步。

- `server-core`：统一错误、分页、Clock、健康检查和幂等基础。
- `job-pipeline`：持久任务状态、lease、重试和安全上下文。
- `platform-connectors`：平台连接器与能力探测。
- `crawler-capture`：文章识别、正文和资产采集。
- `provider-hub`：外部 provider 抽象与 fake 实现。
