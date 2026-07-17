//! 鉴权提取器接口占位（具体实现由各产品在后续模组提供）。

/// 请求主体类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActorType {
    User,
    ApiToken,
    System,
    Worker,
}

/// 审计/鉴权上下文最小字段。
#[derive(Debug, Clone)]
pub struct AuthContext {
    pub actor_type: ActorType,
    pub actor_id: Option<String>,
    pub product: &'static str,
    pub trace_id: String,
}
