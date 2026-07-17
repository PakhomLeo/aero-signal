use crate::config::ProviderKind;
use crate::error::ProviderResult;
use async_trait::async_trait;
use std::collections::HashMap;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct OcrRequest {
    pub image_data: Option<Vec<u8>>,
    pub file_url: Option<String>,
}

#[derive(Debug, Clone)]
pub struct OcrResponse {
    pub markdown_text: String,
    pub images: HashMap<String, Vec<u8>>,
    pub pages: u32,
    /// 0.0–1.0；None 表示 provider 未给出。
    pub confidence: Option<f64>,
    /// Provider-side identity needed to reproduce and audit this result.
    pub trace: OcrTrace,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OcrTrace {
    pub provider: String,
    pub model: String,
    pub version: Option<String>,
    pub request_id: Option<String>,
}

impl OcrTrace {
    pub fn new(provider: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            model: model.into(),
            version: None,
            request_id: None,
        }
    }

    pub fn with_version(mut self, version: impl Into<String>) -> Self {
        self.version = Some(version.into());
        self
    }

    pub fn with_request_id(mut self, request_id: impl Into<String>) -> Self {
        self.request_id = Some(request_id.into());
        self
    }
}

/// Vendor-neutral execution controls. Providers should apply this timeout to
/// their complete remote operation, including upload and result polling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OcrCallOptions {
    pub timeout: Duration,
}

impl Default for OcrCallOptions {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(120),
        }
    }
}

#[async_trait]
pub trait OcrProvider: Send + Sync {
    fn id(&self) -> &str;
    fn kind(&self) -> ProviderKind {
        ProviderKind::Ocr
    }
    async fn recognize(&self, request: OcrRequest) -> ProviderResult<OcrResponse>;
    async fn recognize_with_options(
        &self,
        request: OcrRequest,
        _options: OcrCallOptions,
    ) -> ProviderResult<OcrResponse> {
        self.recognize(request).await
    }
    async fn cancel(&self) -> ProviderResult<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trace_carries_reproducibility_metadata() {
        let trace = OcrTrace::new("acme", "document-ocr")
            .with_version("2026-07")
            .with_request_id("req-42");
        assert_eq!(trace.provider, "acme");
        assert_eq!(trace.model, "document-ocr");
        assert_eq!(trace.version.as_deref(), Some("2026-07"));
        assert_eq!(trace.request_id.as_deref(), Some("req-42"));
    }

    #[test]
    fn call_options_have_a_bounded_default() {
        assert_eq!(OcrCallOptions::default().timeout, Duration::from_secs(120));
    }
}
