//! Signal 裁剪版 provider-hub：仅 OCR trait + FakeOcr。

pub mod config;
pub mod error;
pub mod fake;
pub mod ocr;

pub use config::ProviderKind;
pub use error::{ProviderError, ProviderErrorClass, ProviderResult};
pub use fake::FakeOcrProvider;
pub use ocr::{OcrCallOptions, OcrProvider, OcrRequest, OcrResponse, OcrTrace};
