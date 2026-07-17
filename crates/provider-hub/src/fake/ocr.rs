use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::config::ProviderKind;
use crate::error::{ProviderError, ProviderResult};
use crate::ocr::{OcrProvider, OcrRequest, OcrResponse, OcrTrace};

pub struct FakeOcrProvider {
    id: String,
    call_count: Arc<AtomicU64>,
    fail_next: Arc<AtomicBool>,
    confidence: f64,
    text: String,
}

impl FakeOcrProvider {
    pub fn new(id: &str) -> Self {
        FakeOcrProvider {
            id: id.to_string(),
            call_count: Arc::new(AtomicU64::new(0)),
            fail_next: Arc::new(AtomicBool::new(false)),
            confidence: 0.92,
            text: "Fake OCR content".into(),
        }
    }

    pub fn with_text(mut self, text: impl Into<String>) -> Self {
        self.text = text.into();
        self
    }

    pub fn with_confidence(mut self, confidence: f64) -> Self {
        self.confidence = confidence;
        self
    }

    pub fn call_count(&self) -> u64 {
        self.call_count.load(Ordering::Relaxed)
    }

    pub fn fail_next(&self) {
        self.fail_next.store(true, Ordering::Relaxed);
    }
}

#[async_trait]
impl OcrProvider for FakeOcrProvider {
    fn id(&self) -> &str {
        &self.id
    }

    fn kind(&self) -> ProviderKind {
        ProviderKind::Ocr
    }

    async fn recognize(&self, _request: OcrRequest) -> ProviderResult<OcrResponse> {
        self.call_count.fetch_add(1, Ordering::Relaxed);
        if self.fail_next.swap(false, Ordering::Relaxed) {
            return Err(ProviderError::permanent(
                "provider_hub.ocr_failed",
                "fake OCR forced failure",
            ));
        }
        Ok(OcrResponse {
            markdown_text: self.text.clone(),
            images: HashMap::new(),
            pages: 1,
            confidence: Some(self.confidence),
            trace: OcrTrace::new(self.id.clone(), "fake-ocr").with_version("1"),
        })
    }

    async fn cancel(&self) -> ProviderResult<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ocr::OcrCallOptions;

    #[tokio::test]
    async fn fake_response_identifies_its_provider_and_model() {
        let provider = FakeOcrProvider::new("test-ocr");
        let response = provider
            .recognize_with_options(
                OcrRequest {
                    image_data: Some(vec![1, 2, 3]),
                    file_url: None,
                },
                OcrCallOptions::default(),
            )
            .await
            .expect("fake OCR response");

        assert_eq!(response.trace.provider, "test-ocr");
        assert_eq!(response.trace.model, "fake-ocr");
        assert_eq!(response.trace.version.as_deref(), Some("1"));
        assert_eq!(provider.call_count(), 1);
    }
}
