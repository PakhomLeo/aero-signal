//! OpenAPI 预留入口。首期契约以 `docs/api/OpenAPI与契约冻结.md` + 集成测试为准；
//! 全量 YAML/utoipa 生成为可选后续，非硬门槛。

/// OpenAPI 文档元信息。
#[derive(Debug, Clone)]
pub struct OpenApiMeta {
    pub title: &'static str,
    pub version: &'static str,
    pub description: &'static str,
}

impl OpenApiMeta {
    pub const SIGNAL: Self = Self {
        title: "Aero Signal API",
        version: "0.0.1",
        description: "Aero Signal 管理端与投递 API（骨架）",
    };
}

/// 预留：后续用 utoipa / 手工 schema 生成完整 OpenAPI。
pub fn openapi_stub_json(meta: &OpenApiMeta) -> serde_json::Value {
    serde_json::json!({
        "openapi": "3.1.0",
        "info": {
            "title": meta.title,
            "version": meta.version,
            "description": meta.description
        },
        "paths": {
            "/healthz": {
                "get": {
                    "summary": "进程级健康检查",
                    "operationId": "healthz",
                    "responses": {
                        "200": {
                            "description": "进程存活"
                        }
                    }
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openapi_stub_has_version_and_healthz() {
        let doc = openapi_stub_json(&OpenApiMeta::SIGNAL);
        assert_eq!(doc["info"]["version"], "0.0.1");
        assert!(doc["paths"]["/healthz"]["get"].is_object());
    }
}
