//! Production article acquisition: bounded HTTP first, with one optional CDP fallback.
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use reqwest::{Client, Proxy, StatusCode, header::LOCATION};
use url::Url;

use crate::browser::{
    BrowserRenderRequest, BrowserRuntime, DisabledBrowser, merge_lazy_candidates,
};
#[cfg(not(test))]
use crate::egress_guard::validate_public_http_url;
use crate::error::{CrawlerError, CrawlerResult};
use crate::extractor::{BodyExtractor, ExtractedBody};
use crate::types::{CaptureRequest, CaptureResult, CaptureSource};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(15);
const DEFAULT_MAX_HTML_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone)]
pub struct PageFetchRequest {
    pub url: String,
    pub proxy_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchedPage {
    pub effective_url: String,
    pub html: String,
}

impl std::fmt::Debug for PageFetchRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PageFetchRequest")
            .field("url", &self.url)
            .field("proxy_url", &self.proxy_url.as_ref().map(|_| "[REDACTED]"))
            .finish()
    }
}

#[async_trait]
pub trait PageFetcher: Send + Sync {
    async fn fetch_html(&self, request: &PageFetchRequest) -> CrawlerResult<String>;

    async fn fetch_page(&self, request: &PageFetchRequest) -> CrawlerResult<FetchedPage> {
        Ok(FetchedPage {
            effective_url: request.url.clone(),
            html: self.fetch_html(request).await?,
        })
    }
}

/// Bounded production HTTP fetcher. Clients are cached per proxy without logging credentials.
pub struct ReqwestPageFetcher {
    timeout: Duration,
    max_html_bytes: usize,
    direct: Client,
    proxy_clients: Mutex<HashMap<String, Client>>,
}

impl ReqwestPageFetcher {
    pub fn new(timeout: Duration, max_html_bytes: usize) -> CrawlerResult<Self> {
        let direct = build_client(timeout, None)?;
        Ok(Self {
            timeout,
            max_html_bytes,
            direct,
            proxy_clients: Mutex::new(HashMap::new()),
        })
    }

    fn client(&self, proxy_url: Option<&str>) -> CrawlerResult<Client> {
        let Some(proxy_url) = proxy_url else {
            return Ok(self.direct.clone());
        };
        if let Some(client) = self
            .proxy_clients
            .lock()
            .expect("proxy cache")
            .get(proxy_url)
        {
            return Ok(client.clone());
        }
        let client = build_client(self.timeout, Some(proxy_url))?;
        self.proxy_clients
            .lock()
            .expect("proxy cache")
            .insert(proxy_url.to_owned(), client.clone());
        Ok(client)
    }
}

impl Default for ReqwestPageFetcher {
    fn default() -> Self {
        Self::new(DEFAULT_TIMEOUT, DEFAULT_MAX_HTML_BYTES)
            .expect("default reqwest page client must be valid")
    }
}

fn build_client(timeout: Duration, proxy_url: Option<&str>) -> CrawlerResult<Client> {
    let mut builder = Client::builder()
        .timeout(timeout)
        // Redirects are followed explicitly so every hop can be revalidated
        // against the SSRF policy before the next request is sent.
        .redirect(reqwest::redirect::Policy::none());
    if let Some(proxy_url) = proxy_url {
        let proxy = Proxy::all(proxy_url).map_err(|_| {
            CrawlerError::new(
                "crawler.proxy_invalid",
                "fixed-egress proxy configuration is invalid",
                false,
            )
        })?;
        builder = builder.proxy(proxy);
    }
    builder.build().map_err(|_| {
        CrawlerError::new(
            "crawler.http_client_invalid",
            "HTTP client initialization failed",
            false,
        )
    })
}

#[async_trait]
impl PageFetcher for ReqwestPageFetcher {
    async fn fetch_html(&self, request: &PageFetchRequest) -> CrawlerResult<String> {
        Ok(self.fetch_page(request).await?.html)
    }

    async fn fetch_page(&self, request: &PageFetchRequest) -> CrawlerResult<FetchedPage> {
        let parsed = Url::parse(&request.url).map_err(|_| {
            CrawlerError::new("crawler.url_invalid", "article URL is invalid", false)
        })?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(CrawlerError::new(
                "crawler.url_scheme_unsupported",
                "article URL must use http or https",
                false,
            ));
        }
        let client = self.client(request.proxy_url.as_deref())?;
        let mut current_url = parsed;
        let response = {
            let mut redirects = 0_u8;
            loop {
                validate_fetch_target(&current_url).await?;
                let response = client.get(current_url.clone()).send().await.map_err(|e| {
                    let code = if e.is_timeout() {
                        "crawler.http_timeout"
                    } else {
                        "crawler.http_failed"
                    };
                    CrawlerError::new(code, "article HTTP request failed", true)
                })?;
                if response.status().is_redirection() {
                    redirects = redirects.saturating_add(1);
                    if redirects > 5 {
                        return Err(CrawlerError::new(
                            "crawler.redirect_limit",
                            "too many article redirects",
                            false,
                        ));
                    }
                    let location = response
                        .headers()
                        .get(LOCATION)
                        .and_then(|value| value.to_str().ok())
                        .ok_or_else(|| {
                            CrawlerError::new(
                                "crawler.redirect_invalid",
                                "article redirect has no valid location",
                                false,
                            )
                        })?;
                    current_url = current_url.join(location).map_err(|_| {
                        CrawlerError::new(
                            "crawler.redirect_invalid",
                            "article redirect URL is invalid",
                            false,
                        )
                    })?;
                    continue;
                }
                break response;
            }
        };
        let status = response.status();
        if status == StatusCode::TOO_MANY_REQUESTS {
            return Err(CrawlerError::new(
                "crawler.rate_limited",
                "article host rate limited the request",
                true,
            ));
        }
        if status == StatusCode::FORBIDDEN {
            return Err(CrawlerError::blocked_by_protection(
                "article host rejected the request",
            ));
        }
        if !status.is_success() {
            return Err(CrawlerError::new(
                "crawler.http_status",
                format!("article host returned HTTP {}", status.as_u16()),
                status.is_server_error(),
            ));
        }
        if response
            .content_length()
            .is_some_and(|n| n > self.max_html_bytes as u64)
        {
            return Err(CrawlerError::new(
                "crawler.html_too_large",
                "article HTML exceeds the configured limit",
                false,
            ));
        }
        let bytes = response.bytes().await.map_err(|_| {
            CrawlerError::new(
                "crawler.http_body_failed",
                "article HTTP body could not be read",
                true,
            )
        })?;
        if bytes.len() > self.max_html_bytes {
            return Err(CrawlerError::new(
                "crawler.html_too_large",
                "article HTML exceeds the configured limit",
                false,
            ));
        }
        let html = String::from_utf8_lossy(&bytes).into_owned();
        if looks_like_login_challenge(&html) {
            return Err(CrawlerError::new(
                "crawler.login_challenge",
                "article host requires an interactive login or verification",
                true,
            ));
        }
        Ok(FetchedPage {
            effective_url: current_url.to_string(),
            html,
        })
    }
}

fn looks_like_login_challenge(html: &str) -> bool {
    let lower = html.to_ascii_lowercase();
    [
        "verify you are human",
        "captcha",
        "access denied",
        "login required",
        "请完成验证",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

async fn validate_fetch_target(url: &Url) -> CrawlerResult<()> {
    #[cfg(test)]
    {
        let _ = url;
        Ok(())
    }
    #[cfg(not(test))]
    validate_public_http_url(url).await
}

#[derive(Default)]
pub struct MemoryPageFetcher {
    responses: Mutex<HashMap<String, CrawlerResult<FetchedPage>>>,
    calls: Mutex<Vec<PageFetchRequest>>,
}

impl MemoryPageFetcher {
    pub fn insert_html(&self, url: impl Into<String>, html: impl Into<String>) {
        let url = url.into();
        self.responses.lock().expect("responses").insert(
            url.clone(),
            Ok(FetchedPage {
                effective_url: url,
                html: html.into(),
            }),
        );
    }

    pub fn insert_page(
        &self,
        url: impl Into<String>,
        effective_url: impl Into<String>,
        html: impl Into<String>,
    ) {
        self.responses.lock().expect("responses").insert(
            url.into(),
            Ok(FetchedPage {
                effective_url: effective_url.into(),
                html: html.into(),
            }),
        );
    }

    pub fn insert_error(&self, url: impl Into<String>, error: CrawlerError) {
        self.responses
            .lock()
            .expect("responses")
            .insert(url.into(), Err(error));
    }

    pub fn call_count(&self) -> usize {
        self.calls.lock().expect("calls").len()
    }
}

#[async_trait]
impl PageFetcher for MemoryPageFetcher {
    async fn fetch_html(&self, request: &PageFetchRequest) -> CrawlerResult<String> {
        Ok(self.fetch_page(request).await?.html)
    }

    async fn fetch_page(&self, request: &PageFetchRequest) -> CrawlerResult<FetchedPage> {
        self.calls.lock().expect("calls").push(request.clone());
        self.responses
            .lock()
            .expect("responses")
            .get(&request.url)
            .cloned()
            .unwrap_or_else(|| Err(CrawlerError::fetch_failed("memory page response missing")))
    }
}

pub struct ArticleFetchClient {
    extractor: BodyExtractor,
    page_fetcher: Arc<dyn PageFetcher>,
    browser: Arc<dyn BrowserRuntime>,
}

impl Default for ArticleFetchClient {
    fn default() -> Self {
        Self {
            extractor: BodyExtractor::new(),
            page_fetcher: Arc::new(ReqwestPageFetcher::default()),
            browser: Arc::new(DisabledBrowser),
        }
    }
}

impl ArticleFetchClient {
    /// Retained as the browser injection constructor; HTTP is always real unless explicitly replaced.
    pub fn new(browser: Arc<dyn BrowserRuntime>) -> Self {
        Self::default().with_browser(browser)
    }

    pub fn with_page_fetcher(mut self, page_fetcher: Arc<dyn PageFetcher>) -> Self {
        self.page_fetcher = page_fetcher;
        self
    }

    pub fn with_browser(mut self, browser: Arc<dyn BrowserRuntime>) -> Self {
        self.browser = browser;
        self
    }

    pub async fn fetch_and_extract(&self, req: &CaptureRequest) -> CrawlerResult<CaptureResult> {
        let http = self
            .page_fetcher
            .fetch_html(&PageFetchRequest {
                url: req.url.clone(),
                proxy_url: req.proxy_url.clone(),
            })
            .await;

        if let Ok(html) = &http
            && let Ok(extracted) = self.extractor.extract(html, &req.url)
            && !requires_browser(&extracted)
        {
            return build_result(extracted, html.clone(), CaptureSource::HttpDirect, false);
        }

        if let Err(error) = &http
            && matches!(
                error.code.as_str(),
                "crawler.rate_limited"
                    | "crawler.blocked_by_protection"
                    | "crawler.login_challenge"
            )
        {
            // 平台限制必须直接进入 needs_action，禁止借浏览器兜底规避。
            return Err(error.clone());
        }

        if !self.browser.supports_network_fetch() {
            return match http {
                Err(error) => Err(error),
                Ok(_) => Err(CrawlerError::new(
                    "crawler.browser_required",
                    "article body is incomplete and browser rendering is disabled",
                    false,
                )),
            };
        }

        let rendered = self
            .browser
            .render_for_lazy_images(&BrowserRenderRequest {
                url: req.url.clone(),
                scroll_container_selector: "#js_content".into(),
                html_hint: http.ok(),
                proxy_url: req.proxy_url.clone(),
            })
            .await?;
        let mut extracted = self.extractor.extract(&rendered.html, &req.url)?;
        merge_lazy_candidates(&mut extracted.candidates, &rendered.dom_images, true);
        build_result(extracted, rendered.html, CaptureSource::BrowserRender, true)
    }
}

fn requires_browser(extracted: &ExtractedBody) -> bool {
    extracted.needs_lazy_render || extracted.text_length < 20
}

fn build_result(
    extracted: ExtractedBody,
    raw_html: String,
    source: CaptureSource,
    used_browser: bool,
) -> CrawlerResult<CaptureResult> {
    if extracted.text_length == 0 || extracted.content_html.trim().is_empty() {
        return Err(CrawlerError::empty_article("article body is empty"));
    }
    Ok(CaptureResult {
        title: extracted.title,
        author: extracted.author,
        published_at: None,
        content_html: extracted.content_html,
        content_md: extracted.content_md,
        candidates: extracted.candidates,
        raw_html: Some(raw_html),
        source,
        used_browser,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::{BrowserRenderResult, DomImage};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    const URL: &str = "https://example.test/article";
    const COMPLETE: &str = r#"<html><head><title>T</title></head><body><div id="js_content"><p>This is a complete article body with enough meaningful text.</p></div></body></html>"#;

    #[derive(Default)]
    struct MemoryBrowser {
        calls: Mutex<usize>,
        result: Mutex<Option<CrawlerResult<BrowserRenderResult>>>,
    }

    #[async_trait]
    impl BrowserRuntime for MemoryBrowser {
        async fn render_for_lazy_images(
            &self,
            _: &BrowserRenderRequest,
        ) -> CrawlerResult<BrowserRenderResult> {
            *self.calls.lock().unwrap() += 1;
            self.result.lock().unwrap().take().unwrap()
        }
        fn supports_network_fetch(&self) -> bool {
            true
        }
    }

    fn request() -> CaptureRequest {
        CaptureRequest {
            url: URL.into(),
            need_assets: true,
            platform_hint: None,
            proxy_url: None,
        }
    }

    #[tokio::test]
    async fn complete_http_is_direct_and_skips_browser() {
        let pages = Arc::new(MemoryPageFetcher::default());
        pages.insert_html(URL, COMPLETE);
        let browser = Arc::new(MemoryBrowser::default());
        let result = ArticleFetchClient::default()
            .with_page_fetcher(pages)
            .with_browser(browser.clone())
            .fetch_and_extract(&request())
            .await
            .unwrap();
        assert_eq!(result.source, CaptureSource::HttpDirect);
        assert_eq!(*browser.calls.lock().unwrap(), 0);
    }

    #[tokio::test]
    async fn incomplete_http_falls_back_exactly_once() {
        let pages = Arc::new(MemoryPageFetcher::default());
        pages.insert_html(URL, r#"<div id="js_content"><img data-src="x.jpg"></div>"#);
        let browser = Arc::new(MemoryBrowser::default());
        *browser.result.lock().unwrap() = Some(Ok(BrowserRenderResult {
            html: COMPLETE.into(),
            dom_images: vec![DomImage {
                url: "https://cdn/x.jpg".into(),
                natural_width: Some(640),
                natural_height: Some(480),
            }],
        }));
        let result = ArticleFetchClient::default()
            .with_page_fetcher(pages)
            .with_browser(browser.clone())
            .fetch_and_extract(&request())
            .await
            .unwrap();
        assert_eq!(result.source, CaptureSource::BrowserRender);
        assert_eq!(*browser.calls.lock().unwrap(), 1);
    }

    #[tokio::test]
    async fn disabled_browser_fails_for_incomplete_body() {
        let pages = Arc::new(MemoryPageFetcher::default());
        pages.insert_html(URL, "<html></html>");
        let error = ArticleFetchClient::default()
            .with_page_fetcher(pages)
            .fetch_and_extract(&request())
            .await
            .unwrap_err();
        assert_eq!(error.code, "crawler.browser_required");
    }

    #[tokio::test]
    async fn platform_restriction_never_falls_back_to_browser() {
        let pages = Arc::new(MemoryPageFetcher::default());
        pages.insert_error(
            URL,
            CrawlerError::blocked_by_protection("platform challenge"),
        );
        let browser = Arc::new(MemoryBrowser::default());
        let error = ArticleFetchClient::default()
            .with_page_fetcher(pages)
            .with_browser(browser.clone())
            .fetch_and_extract(&request())
            .await
            .unwrap_err();
        assert_eq!(error.code, "crawler.blocked_by_protection");
        assert_eq!(*browser.calls.lock().unwrap(), 0);
    }

    #[tokio::test]
    async fn unsupported_scheme_is_rejected() {
        let fetcher = ReqwestPageFetcher::default();
        let error = fetcher
            .fetch_html(&PageFetchRequest {
                url: "file:///secret".into(),
                proxy_url: None,
            })
            .await
            .unwrap_err();
        assert_eq!(error.code, "crawler.url_scheme_unsupported");
    }

    #[test]
    fn proxy_debug_is_redacted() {
        let debug = format!(
            "{:?}",
            PageFetchRequest {
                url: URL.into(),
                proxy_url: Some("http://user:secret@proxy".into())
            }
        );
        assert!(!debug.contains("secret"));
    }

    async fn one_shot_server(status: &str, body: &str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 1024];
            let _ = socket.read(&mut request).await;
            socket.write_all(response.as_bytes()).await.unwrap();
        });
        format!("http://{address}/article")
    }

    #[tokio::test]
    async fn http_status_and_challenge_have_stable_sanitized_errors() {
        for (status, body, expected) in [
            ("429 Too Many Requests", "slow down", "crawler.rate_limited"),
            (
                "403 Forbidden",
                "secret upstream response",
                "crawler.blocked_by_protection",
            ),
            ("200 OK", "<html>captcha</html>", "crawler.login_challenge"),
        ] {
            let url = one_shot_server(status, body).await;
            let error = ReqwestPageFetcher::default()
                .fetch_html(&PageFetchRequest {
                    url,
                    proxy_url: None,
                })
                .await
                .unwrap_err();
            assert_eq!(error.code, expected);
            assert!(!error.message.contains("secret upstream response"));
        }
    }

    #[tokio::test]
    async fn html_size_limit_is_enforced() {
        let url = one_shot_server("200 OK", "123456789").await;
        let error = ReqwestPageFetcher::new(Duration::from_secs(1), 8)
            .unwrap()
            .fetch_html(&PageFetchRequest {
                url,
                proxy_url: None,
            })
            .await
            .unwrap_err();
        assert_eq!(error.code, "crawler.html_too_large");
    }
}
