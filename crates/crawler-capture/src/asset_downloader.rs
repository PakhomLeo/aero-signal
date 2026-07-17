//! Download, validate, measure, and persist article image assets.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::header::{CONTENT_LENGTH, CONTENT_TYPE};

use crate::classifier::MeasuredImage;
#[cfg(not(test))]
use crate::egress_guard::validate_public_http_url;
use crate::error::{CrawlerError, CrawlerResult};
use crate::object_store::ObjectStore;
use crate::types::{ImageDecision, ImageDecisionKind, StoredObject};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(20);
const DEFAULT_MAX_BYTES: usize = 20 * 1024 * 1024;

#[derive(Clone)]
pub struct AssetFetchRequest {
    pub url: String,
    pub proxy_url: Option<String>,
}

impl AssetFetchRequest {
    pub fn direct(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            proxy_url: None,
        }
    }
}

impl fmt::Debug for AssetFetchRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AssetFetchRequest")
            .field("url", &self.url)
            .field("proxy_url", &self.proxy_url.as_ref().map(|_| "[REDACTED]"))
            .finish()
    }
}

#[derive(Debug, Clone)]
pub struct FetchedAsset {
    pub bytes: Vec<u8>,
    pub content_type: String,
    pub final_url: String,
    pub width: u32,
    pub height: u32,
}

impl FetchedAsset {
    pub fn measured(&self) -> MeasuredImage {
        MeasuredImage {
            width: self.width,
            height: self.height,
            mime_type: Some(self.content_type.clone()),
            bytes: self.bytes.clone(),
        }
    }
}

#[async_trait]
pub trait ByteFetcher: Send + Sync {
    async fn fetch(&self, request: &AssetFetchRequest) -> CrawlerResult<FetchedAsset>;
}

#[derive(Default, Clone)]
pub struct MemoryByteFetcher {
    map: Arc<tokio::sync::RwLock<HashMap<String, Result<FetchedAsset, CrawlerError>>>>,
}

impl MemoryByteFetcher {
    pub fn new() -> Self {
        Self::default()
    }

    /// Inserts explicit, already measured fixture metadata. Tests must not rely on URL suffixes.
    pub async fn insert_asset(&self, url: impl Into<String>, asset: FetchedAsset) {
        self.map.write().await.insert(url.into(), Ok(asset));
    }

    pub async fn insert_error(&self, url: impl Into<String>, error: CrawlerError) {
        self.map.write().await.insert(url.into(), Err(error));
    }

    /// Compatibility helper which decodes real image bytes and records their measured metadata.
    pub async fn insert(&self, url: impl Into<String>, bytes: Vec<u8>) -> CrawlerResult<()> {
        let url = url.into();
        let (content_type, width, height) = decode_image_metadata(&bytes)?;
        self.insert_asset(
            url.clone(),
            FetchedAsset {
                bytes,
                content_type,
                final_url: url,
                width,
                height,
            },
        )
        .await;
        Ok(())
    }
}

#[async_trait]
impl ByteFetcher for MemoryByteFetcher {
    async fn fetch(&self, request: &AssetFetchRequest) -> CrawlerResult<FetchedAsset> {
        self.map
            .read()
            .await
            .get(&request.url)
            .cloned()
            .unwrap_or_else(|| {
                Err(CrawlerError::fetch_failed(format!(
                    "missing asset fixture: {}",
                    request.url
                )))
            })
    }
}

#[derive(Debug, Clone)]
pub struct ReqwestByteFetcher {
    timeout: Duration,
    max_bytes: usize,
}

impl Default for ReqwestByteFetcher {
    fn default() -> Self {
        Self::new(DEFAULT_TIMEOUT, DEFAULT_MAX_BYTES)
    }
}

impl ReqwestByteFetcher {
    pub fn new(timeout: Duration, max_bytes: usize) -> Self {
        Self {
            timeout,
            max_bytes: max_bytes.max(1),
        }
    }

    fn client(&self, proxy_url: Option<&str>) -> CrawlerResult<reqwest::Client> {
        let mut builder = reqwest::Client::builder()
            .timeout(self.timeout)
            // Redirects are followed explicitly so every hop is revalidated.
            .redirect(reqwest::redirect::Policy::none());
        if let Some(proxy_url) = proxy_url {
            let proxy = reqwest::Proxy::all(proxy_url)
                .map_err(|_| CrawlerError::fetch_failed("invalid asset proxy configuration"))?;
            builder = builder.proxy(proxy);
        }
        builder
            .build()
            .map_err(|_| CrawlerError::fetch_failed("failed to initialize asset HTTP client"))
    }
}

#[async_trait]
impl ByteFetcher for ReqwestByteFetcher {
    async fn fetch(&self, request: &AssetFetchRequest) -> CrawlerResult<FetchedAsset> {
        let parsed = url::Url::parse(&request.url)
            .map_err(|_| CrawlerError::fetch_failed("invalid asset URL"))?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(CrawlerError::fetch_failed(
                "asset URL scheme must be http or https",
            ));
        }

        let client = self.client(request.proxy_url.as_deref())?;
        let mut current_url = parsed;
        let mut redirects = 0_u8;
        let mut response = loop {
            validate_fetch_target(&current_url).await?;
            let response = client
                .get(current_url.clone())
                .send()
                .await
                .map_err(|_| CrawlerError::fetch_failed("asset request failed"))?;
            if response.status().is_redirection() {
                redirects = redirects.saturating_add(1);
                if redirects > 5 {
                    return Err(CrawlerError::new(
                        "crawler.redirect_limit",
                        "too many asset redirects",
                        false,
                    ));
                }
                let location = response
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|value| value.to_str().ok())
                    .ok_or_else(|| {
                        CrawlerError::fetch_failed("asset redirect has no valid location")
                    })?;
                current_url = current_url
                    .join(location)
                    .map_err(|_| CrawlerError::fetch_failed("asset redirect URL is invalid"))?;
                continue;
            }
            break response;
        };
        if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(CrawlerError::new(
                "crawler.rate_limited",
                "asset host rate limited the request",
                true,
            ));
        }
        if response.status() == reqwest::StatusCode::FORBIDDEN {
            return Err(CrawlerError::blocked_by_protection(
                "asset host rejected the request",
            ));
        }
        if !response.status().is_success() {
            return Err(CrawlerError::new(
                "crawler.asset_http_status",
                format!("asset host returned HTTP {}", response.status().as_u16()),
                response.status().is_server_error(),
            ));
        }
        let declared_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(';').next())
            .map(str::trim)
            .filter(|v| v.to_ascii_lowercase().starts_with("image/"))
            .ok_or_else(|| CrawlerError::image_filtered("asset Content-Type is not image/*"))?;
        if declared_type.eq_ignore_ascii_case("image/svg+xml") {
            return Err(CrawlerError::image_filtered(
                "vector images are not supported article assets",
            ));
        }
        if response
            .headers()
            .get(CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok())
            .is_some_and(|n| n > self.max_bytes as u64)
        {
            return Err(CrawlerError::image_filtered("asset exceeds byte limit"));
        }

        validate_fetch_target(response.url()).await?;
        let final_url = response.url().to_string();
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| CrawlerError::fetch_failed("asset body read failed"))?
        {
            if bytes.len().saturating_add(chunk.len()) > self.max_bytes {
                return Err(CrawlerError::image_filtered("asset exceeds byte limit"));
            }
            bytes.extend_from_slice(&chunk);
        }
        let (content_type, width, height) = decode_image_metadata(&bytes)?;
        Ok(FetchedAsset {
            bytes,
            content_type,
            final_url,
            width,
            height,
        })
    }
}

pub struct AssetDownloader {
    store: Arc<dyn ObjectStore>,
    fetcher: Arc<dyn ByteFetcher>,
}

impl AssetDownloader {
    pub fn new(store: Arc<dyn ObjectStore>, fetcher: Arc<dyn ByteFetcher>) -> Self {
        Self { store, fetcher }
    }

    pub async fn fetch(
        &self,
        url: impl Into<String>,
        proxy_url: Option<String>,
    ) -> CrawlerResult<FetchedAsset> {
        self.fetcher
            .fetch(&AssetFetchRequest {
                url: url.into(),
                proxy_url,
            })
            .await
    }

    /// Persists arbitrary bytes through the configured object store.
    pub async fn store_bytes(
        &self,
        key_prefix: &str,
        bytes: &[u8],
        mime_type: Option<&str>,
    ) -> CrawlerResult<StoredObject> {
        self.store
            .put_bytes(key_prefix, bytes, mime_type, None, None)
            .await
    }

    pub async fn store_raw_html(
        &self,
        article_key_prefix: &str,
        html: &str,
    ) -> CrawlerResult<StoredObject> {
        self.store_bytes(
            &format!("{article_key_prefix}/raw"),
            html.as_bytes(),
            Some("text/html; charset=utf-8"),
        )
        .await
    }

    /// 保存已经下载并完成格式/尺寸校验的图片，避免分类后重复请求远端。
    pub async fn store_fetched(
        &self,
        article_key_prefix: &str,
        fetched: &FetchedAsset,
    ) -> CrawlerResult<StoredObject> {
        self.store
            .put_bytes(
                article_key_prefix,
                &fetched.bytes,
                Some(&fetched.content_type),
                Some(fetched.width),
                Some(fetched.height),
            )
            .await
    }

    /// Downloads kept decisions using the same fixed-egress proxy as article capture.
    /// Download failures are returned as explicit decisions while successful assets continue.
    pub async fn download_kept_with_proxy(
        &self,
        article_key_prefix: &str,
        decisions: &[ImageDecision],
        proxy_url: Option<String>,
    ) -> Vec<AssetDownloadOutcome> {
        let mut out = Vec::new();
        for decision in decisions {
            if decision.decision != ImageDecisionKind::Kept {
                continue;
            }
            let fetched = match self
                .fetch(decision.candidate_url.clone(), proxy_url.clone())
                .await
            {
                Ok(fetched) => fetched,
                Err(error) => {
                    out.push(AssetDownloadOutcome::Rejected(download_failure_decision(
                        decision, &error,
                    )));
                    continue;
                }
            };
            match self
                .store
                .put_bytes(
                    article_key_prefix,
                    &fetched.bytes,
                    Some(&fetched.content_type),
                    Some(fetched.width),
                    Some(fetched.height),
                )
                .await
            {
                Ok(stored) => out.push(AssetDownloadOutcome::Stored {
                    decision: measured_decision(decision, &fetched),
                    stored,
                    measured: fetched.measured(),
                }),
                Err(error) => out.push(AssetDownloadOutcome::StoreFailed {
                    decision: measured_decision(decision, &fetched),
                    error,
                }),
            }
        }
        out
    }

    /// Compatibility adapter. Real dimensions and MIME always come from decoded bytes;
    /// caller-provided size hints are intentionally ignored.
    pub async fn download_kept(
        &self,
        article_key_prefix: &str,
        decisions: &[ImageDecision],
        _size_hints: &HashMap<String, (u32, u32)>,
    ) -> CrawlerResult<Vec<(ImageDecision, StoredObject, MeasuredImage)>> {
        let outcomes = self
            .download_kept_with_proxy(article_key_prefix, decisions, None)
            .await;
        let mut stored = Vec::new();
        for outcome in outcomes {
            match outcome {
                AssetDownloadOutcome::Stored {
                    decision,
                    stored: object,
                    measured,
                } => stored.push((decision, object, measured)),
                AssetDownloadOutcome::Rejected(_) => {}
                AssetDownloadOutcome::StoreFailed { error, .. } => return Err(error),
            }
        }
        Ok(stored)
    }
}

#[derive(Debug)]
pub enum AssetDownloadOutcome {
    Stored {
        decision: ImageDecision,
        stored: StoredObject,
        measured: MeasuredImage,
    },
    Rejected(ImageDecision),
    StoreFailed {
        decision: ImageDecision,
        error: CrawlerError,
    },
}

fn measured_decision(original: &ImageDecision, fetched: &FetchedAsset) -> ImageDecision {
    let mut decision = original.clone();
    decision.measured_width = Some(fetched.width);
    decision.measured_height = Some(fetched.height);
    decision.reason_json["measured_content_type"] = serde_json::json!(fetched.content_type);
    decision.reason_json["final_url"] = serde_json::json!(fetched.final_url);
    decision
}

fn download_failure_decision(original: &ImageDecision, error: &CrawlerError) -> ImageDecision {
    ImageDecision {
        candidate_url: original.candidate_url.clone(),
        decision: if error.code == "image.filtered" {
            ImageDecisionKind::Unsupported
        } else {
            ImageDecisionKind::DownloadFailed
        },
        measured_width: None,
        measured_height: None,
        reason_json: serde_json::json!({
            "reason": if error.code == "image.filtered" { "unsupported_download" } else { "download_failed" },
            "error_code": error.code,
            "retryable": error.retryable,
        }),
    }
}

fn decode_image_metadata(bytes: &[u8]) -> CrawlerResult<(String, u32, u32)> {
    if bytes.is_empty() {
        return Err(CrawlerError::image_filtered("empty image body"));
    }
    let format = image::guess_format(bytes)
        .map_err(|_| CrawlerError::image_filtered("asset body is not a supported image"))?;
    let content_type = match format {
        image::ImageFormat::Png => "image/png",
        image::ImageFormat::Jpeg => "image/jpeg",
        image::ImageFormat::Gif => "image/gif",
        image::ImageFormat::WebP => "image/webp",
        _ => {
            return Err(CrawlerError::image_filtered(
                "decoded image format is not supported",
            ));
        }
    };
    let (width, height) = image::ImageReader::with_format(std::io::Cursor::new(bytes), format)
        .into_dimensions()
        .map_err(|_| CrawlerError::image_filtered("failed to decode image dimensions"))?;
    if width == 0 || height == 0 {
        return Err(CrawlerError::image_filtered("image has zero dimensions"));
    }
    Ok((content_type.to_string(), width, height))
}

async fn validate_fetch_target(url: &url::Url) -> CrawlerResult<()> {
    #[cfg(test)]
    {
        let _ = url;
        Ok(())
    }
    #[cfg(not(test))]
    validate_public_http_url(url).await
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};

    use super::*;
    use crate::object_store::FakeObjectStore;

    struct FailingStore;

    #[async_trait]
    impl ObjectStore for FailingStore {
        async fn put_bytes(
            &self,
            _key_prefix: &str,
            _bytes: &[u8],
            _mime_type: Option<&str>,
            _width: Option<u32>,
            _height: Option<u32>,
        ) -> CrawlerResult<StoredObject> {
            Err(CrawlerError::store_failed("injected object-store failure"))
        }

        async fn get_bytes(&self, _storage_key: &str) -> CrawlerResult<Option<Vec<u8>>> {
            Ok(None)
        }

        async fn exists_by_hash(&self, _content_hash: &str) -> CrawlerResult<bool> {
            Ok(false)
        }
    }

    fn png(width: u32, height: u32) -> Vec<u8> {
        let image = image::DynamicImage::new_rgba8(width, height);
        let mut bytes = std::io::Cursor::new(Vec::new());
        image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
        bytes.into_inner()
    }

    fn kept(url: &str) -> ImageDecision {
        ImageDecision {
            candidate_url: url.into(),
            decision: ImageDecisionKind::Kept,
            measured_width: Some(999),
            measured_height: Some(999),
            reason_json: serde_json::json!({"reason":"declared"}),
        }
    }

    fn serve_once(content_type: &str, body: Vec<u8>) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let content_type = content_type.to_string();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 2048];
            let _ = stream.read(&mut request);
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .unwrap();
            stream.write_all(&body).unwrap();
        });
        format!("http://{address}/misleading.jpg")
    }

    #[tokio::test]
    async fn uses_real_png_dimensions_and_deduplicates_hash() {
        let fetcher = Arc::new(MemoryByteFetcher::new());
        for url in ["https://cdn/a.noext", "https://cdn/b.jpg"] {
            fetcher.insert(url, png(121, 87)).await.unwrap();
        }
        let store = Arc::new(FakeObjectStore::new());
        let downloader = AssetDownloader::new(store, fetcher);
        let outcomes = downloader
            .download_kept_with_proxy(
                "articles/a",
                &[kept("https://cdn/a.noext"), kept("https://cdn/b.jpg")],
                None,
            )
            .await;
        let first = match &outcomes[0] {
            AssetDownloadOutcome::Stored { stored, .. } => stored,
            other => panic!("unexpected: {other:?}"),
        };
        let second = match &outcomes[1] {
            AssetDownloadOutcome::Stored { stored, .. } => stored,
            other => panic!("unexpected: {other:?}"),
        };
        assert_eq!((first.width, first.height), (Some(121), Some(87)));
        assert_eq!(first.mime_type.as_deref(), Some("image/png"));
        assert!(!first.deduplicated);
        assert!(second.deduplicated);
    }

    #[tokio::test]
    async fn records_non_image_and_download_failure() {
        let fetcher = Arc::new(MemoryByteFetcher::new());
        fetcher
            .insert_error(
                "https://cdn/not-image.jpg",
                CrawlerError::image_filtered("not an image"),
            )
            .await;
        fetcher
            .insert_error(
                "https://cdn/missing.png",
                CrawlerError::fetch_failed("network unavailable"),
            )
            .await;
        let downloader = AssetDownloader::new(Arc::new(FakeObjectStore::new()), fetcher);
        let outcomes = downloader
            .download_kept_with_proxy(
                "articles/a",
                &[
                    kept("https://cdn/not-image.jpg"),
                    kept("https://cdn/missing.png"),
                ],
                None,
            )
            .await;
        assert!(matches!(
            &outcomes[0],
            AssetDownloadOutcome::Rejected(d) if d.decision == ImageDecisionKind::Unsupported
        ));
        assert!(matches!(
            &outcomes[1],
            AssetDownloadOutcome::Rejected(d) if d.decision == ImageDecisionKind::DownloadFailed
        ));
    }

    #[tokio::test]
    async fn preserves_object_store_failure_as_an_outcome() {
        let fetcher = Arc::new(MemoryByteFetcher::new());
        fetcher
            .insert("https://cdn/image", png(120, 90))
            .await
            .unwrap();
        let downloader = AssetDownloader::new(Arc::new(FailingStore), fetcher);
        let outcomes = downloader
            .download_kept_with_proxy("articles/a", &[kept("https://cdn/image")], None)
            .await;
        assert!(matches!(
            &outcomes[0],
            AssetDownloadOutcome::StoreFailed { error, .. }
                if error.code == "crawler.store_failed"
        ));
    }

    #[tokio::test]
    async fn reqwest_fetcher_rejects_non_image_content_type() {
        let url = serve_once("text/html", b"<html>not an image</html>".to_vec());
        let error = ReqwestByteFetcher::default()
            .fetch(&AssetFetchRequest::direct(url))
            .await
            .unwrap_err();
        assert_eq!(error.code, "image.filtered");
    }

    #[tokio::test]
    async fn reqwest_fetcher_decodes_png_instead_of_trusting_url_suffix() {
        let url = serve_once("image/png", png(137, 83));
        let asset = ReqwestByteFetcher::default()
            .fetch(&AssetFetchRequest::direct(url))
            .await
            .unwrap();
        assert_eq!(asset.content_type, "image/png");
        assert_eq!((asset.width, asset.height), (137, 83));
        assert!(asset.final_url.ends_with("/misleading.jpg"));
    }
}
