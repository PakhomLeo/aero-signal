//! 浏览器懒加载兜底（U09-06）：trait + FakeBrowser 桩 + CloakBrowser CDP。

use async_trait::async_trait;
use chromiumoxide::browser::Browser;
use futures::StreamExt;
use reqwest::Client;

use crate::egress_guard::validate_public_http_url;
use crate::error::{CrawlerError, CrawlerResult};
use crate::types::ImageCandidate;

/// 浏览器渲染请求。
#[derive(Clone)]
pub struct BrowserRenderRequest {
    pub url: String,
    /// 滚动正文容器选择器（微信常见 `#js_content`）。
    pub scroll_container_selector: String,
    pub html_hint: Option<String>,
    /// Expected fixed-egress proxy. Never include this value in errors or logs.
    pub proxy_url: Option<String>,
}

impl std::fmt::Debug for BrowserRenderRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BrowserRenderRequest")
            .field("url", &self.url)
            .field("scroll_container_selector", &self.scroll_container_selector)
            .field("html_hint", &self.html_hint.as_ref().map(|_| "[REDACTED]"))
            .field("proxy_url", &self.proxy_url.as_ref().map(|_| "[REDACTED]"))
            .finish()
    }
}

/// 浏览器渲染结果。
#[derive(Debug, Clone)]
pub struct BrowserRenderResult {
    pub html: String,
    pub dom_images: Vec<DomImage>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct DomImage {
    pub url: String,
    pub natural_width: Option<u32>,
    pub natural_height: Option<u32>,
}

/// 浏览器运行时抽象（真实实现可接 CloakBrowser CDP :9222）。
#[async_trait]
pub trait BrowserRuntime: Send + Sync {
    async fn render_for_lazy_images(
        &self,
        req: &BrowserRenderRequest,
    ) -> CrawlerResult<BrowserRenderResult>;

    /// 是否支持无 fixture 的外网页面抓取。
    fn supports_network_fetch(&self) -> bool {
        false
    }
}

#[derive(Debug, Default, Clone)]
pub struct DisabledBrowser;

#[async_trait]
impl BrowserRuntime for DisabledBrowser {
    async fn render_for_lazy_images(
        &self,
        _req: &BrowserRenderRequest,
    ) -> CrawlerResult<BrowserRenderResult> {
        Err(CrawlerError::new(
            "crawler.browser_disabled",
            "browser rendering is not configured",
            false,
        ))
    }
}

/// 从 HTML 收集懒加载 URL 并模拟写回 src。
pub fn render_lazy_from_html(html: &str, scroll_selector: &str) -> BrowserRenderResult {
    let _ = scroll_selector;
    let mut lazy_image_urls = Vec::new();
    for attr in [
        "data-src",
        "data-original",
        "data-lazy-src",
        "data-actualsrc",
    ] {
        let pattern = format!(r#"{attr}=["']([^"']+)["']"#);
        if let Ok(re) = regex::Regex::new(&pattern) {
            for caps in re.captures_iter(html) {
                if let Some(m) = caps.get(1) {
                    lazy_image_urls.push(m.as_str().to_string());
                }
            }
        }
    }
    lazy_image_urls.sort();
    lazy_image_urls.dedup();

    let mut rendered = html.to_string();
    if let Ok(re) = regex::Regex::new(r#"data-src=["']([^"']+)["']"#) {
        rendered = re
            .replace_all(&rendered, r#"src="$1" data-src="$1""#)
            .into_owned();
    }

    BrowserRenderResult {
        html: rendered,
        dom_images: lazy_image_urls
            .into_iter()
            .map(|url| DomImage {
                url,
                natural_width: None,
                natural_height: None,
            })
            .collect(),
    }
}

/// Fake：从 html_hint 中收集 data-src 等懒加载 URL，不连真实浏览器。
#[derive(Debug, Default, Clone)]
pub struct FakeBrowser;

#[async_trait]
impl BrowserRuntime for FakeBrowser {
    async fn render_for_lazy_images(
        &self,
        req: &BrowserRenderRequest,
    ) -> CrawlerResult<BrowserRenderResult> {
        let html = req
            .html_hint
            .as_deref()
            .ok_or_else(|| CrawlerError::fetch_failed("FakeBrowser 需要 html_hint"))?;
        Ok(render_lazy_from_html(html, &req.scroll_container_selector))
    }
}

/// CloakBrowser / Chrome CDP：连接已有浏览器实例抓取页面 HTML。
#[derive(Clone)]
pub struct CdpBrowser {
    cdp_base: String,
    http: Client,
    configured_proxy: Option<String>,
}

impl std::fmt::Debug for CdpBrowser {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CdpBrowser")
            .field("cdp_base", &"[REDACTED]")
            .field(
                "configured_proxy",
                &self.configured_proxy.as_ref().map(|_| "[REDACTED]"),
            )
            .finish_non_exhaustive()
    }
}

impl CdpBrowser {
    pub fn new(cdp_base: impl Into<String>) -> Self {
        Self {
            cdp_base: cdp_base.into().trim_end_matches('/').to_string(),
            http: Client::new(),
            configured_proxy: None,
        }
    }

    /// Declares the proxy used when the external CDP browser was launched.
    pub fn with_configured_proxy(mut self, proxy_url: impl Into<String>) -> Self {
        self.configured_proxy = Some(proxy_url.into());
        self
    }

    async fn websocket_url(&self) -> CrawlerResult<String> {
        let url = format!("{}/json/version", self.cdp_base);
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|_| CrawlerError::fetch_failed("CDP endpoint is unreachable"))?;
        if !resp.status().is_success() {
            return Err(CrawlerError::fetch_failed(format!(
                "CDP /json/version 返回 {}",
                resp.status()
            )));
        }
        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|_| CrawlerError::fetch_failed("CDP version response is invalid"))?;
        body.get("webSocketDebuggerUrl")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .ok_or_else(|| CrawlerError::fetch_failed("CDP 响应缺少 webSocketDebuggerUrl"))
    }

    async fn fetch_page_html(
        &self,
        page_url: &str,
        _scroll_selector: &str,
    ) -> CrawlerResult<BrowserRenderResult> {
        let ws = self.websocket_url().await?;
        let (mut browser, mut handler) = Browser::connect(&ws)
            .await
            .map_err(|_| CrawlerError::fetch_failed("CDP connection failed"))?;

        let handler_task = tokio::spawn(async move { while handler.next().await.is_some() {} });

        let result = async {
            let page = browser
                .new_page(page_url)
                .await
                .map_err(|_| CrawlerError::fetch_failed("CDP page navigation failed"))?;

            // 等待正文容器出现（微信文章常见结构）。
            let _ = page
                .wait_for_navigation()
                .await
                .map_err(|_| CrawlerError::fetch_failed("CDP page navigation timed out"))?;

            // Chromium may follow redirects internally. Validate the effective URL before
            // reading DOM/content so a redirect cannot turn a public article request into a
            // private-network scrape target.
            let effective_url = page
                .evaluate("window.location.href")
                .await
                .map_err(|_| CrawlerError::fetch_failed("CDP effective URL read failed"))?
                .into_value::<String>()
                .map_err(|_| CrawlerError::fetch_failed("CDP effective URL is invalid"))?;
            let effective_url = url::Url::parse(&effective_url).map_err(|_| {
                CrawlerError::new("crawler.url_invalid", "browser URL is invalid", false)
            })?;
            validate_public_http_url(&effective_url).await?;

            // Only the article body is scrolled; never scroll the full document or arbitrary DOM.
            let sel = "#js_content";
            {
                let script = format!(
                    r#"(async () => {{
  const el = document.querySelector({sel_json});
  if (!el) return false;
  for (let i = 0; i < 6; i++) {{
    el.scrollTop = el.scrollHeight;
    await new Promise(r => setTimeout(r, 300));
  }}
  return true;
}})()"#,
                    sel_json =
                        serde_json::to_string(sel).unwrap_or_else(|_| "\"#js_content\"".into())
                );
                let _ = page.evaluate(script).await;
            }

            let html = page.content()
                .await
                .map_err(|_| CrawlerError::fetch_failed("CDP page HTML read failed"))?;
            let script = r#"Array.from(document.querySelectorAll('#js_content img')).map(img => ({
  url: img.currentSrc || img.getAttribute('data-src') || img.getAttribute('data-original') || img.getAttribute('data-lazy-src') || img.getAttribute('data-actualsrc') || ((img.getAttribute('data-srcset') || img.getAttribute('data-lazy-srcset') || '').split(',')[0] || '').trim().split(/\s+/)[0] || img.src || '',
  natural_width: img.naturalWidth || null,
  natural_height: img.naturalHeight || null
})).filter(item => item.url)"#;
            let evaluated = page
                .evaluate(script)
                .await
                .map_err(|_| CrawlerError::fetch_failed("CDP DOM image inspection failed"))?;
            let dom_images = evaluated
                .into_value::<Vec<DomImage>>()
                .map_err(|_| CrawlerError::fetch_failed("CDP DOM image result invalid"))?;
            Ok(BrowserRenderResult { html, dom_images })
        }
        .await;

        let _ = browser.close().await;
        handler_task.abort();
        result
    }
}

#[async_trait]
impl BrowserRuntime for CdpBrowser {
    async fn render_for_lazy_images(
        &self,
        req: &BrowserRenderRequest,
    ) -> CrawlerResult<BrowserRenderResult> {
        if req.proxy_url.is_some() && req.proxy_url != self.configured_proxy {
            return Err(CrawlerError::new(
                "crawler.browser_egress_mismatch",
                "CDP browser is not bound to the requested fixed egress",
                false,
            ));
        }
        // html_hint is intentionally ignored: production CDP always navigates the URL.
        let target = url::Url::parse(&req.url).map_err(|_| {
            CrawlerError::new("crawler.url_invalid", "browser URL is invalid", false)
        })?;
        validate_public_http_url(&target).await?;
        self.fetch_page_html(&req.url, &req.scroll_container_selector)
            .await
    }

    fn supports_network_fetch(&self) -> bool {
        true
    }
}

/// 将浏览器收集到的 URL 合并进候选（标记 lazy）。
pub fn merge_lazy_candidates(
    existing: &mut Vec<ImageCandidate>,
    dom_images: &[DomImage],
    in_body: bool,
) {
    let have: std::collections::HashSet<_> = existing.iter().map(|c| c.url.clone()).collect();
    for image in dom_images {
        let url = &image.url;
        if have.contains(url) {
            if let Some(c) = existing.iter_mut().find(|c| &c.url == url) {
                c.lazy = true;
                c.declared_width = image.natural_width.or(c.declared_width);
                c.declared_height = image.natural_height.or(c.declared_height);
            }
            continue;
        }
        existing.push(ImageCandidate {
            url: url.clone(),
            alt: None,
            in_body,
            hidden: false,
            lazy: true,
            decorative_hint: !in_body,
            declared_width: None,
            declared_height: image.natural_height,
        });
        if let Some(last) = existing.last_mut() {
            last.declared_width = image.natural_width;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fake_browser_collects_lazy_urls() {
        let html = r#"<div id="js_content"><img data-src="https://cdn/a.jpg"/><img data-src="https://cdn/b.jpg"/></div>"#;
        let browser = FakeBrowser;
        let result = browser
            .render_for_lazy_images(&BrowserRenderRequest {
                url: "https://example.com".into(),
                scroll_container_selector: "#js_content".into(),
                html_hint: Some(html.into()),
                proxy_url: None,
            })
            .await
            .unwrap();
        assert_eq!(result.dom_images.len(), 2);
        assert!(result.html.contains(r#"src="https://cdn/a.jpg""#));
    }

    #[tokio::test]
    async fn cdp_browser_never_uses_html_hint_as_a_render_result() {
        let browser = CdpBrowser::new("http://127.0.0.1:1");
        let error = browser
            .render_for_lazy_images(&BrowserRenderRequest {
                url: "https://example.com/article".into(),
                scroll_container_selector: "body".into(),
                html_hint: Some("<div id='js_content'>hint must not win</div>".into()),
                proxy_url: None,
            })
            .await
            .unwrap_err();
        assert_eq!(error.code, "crawler.fetch_failed");
    }
}
