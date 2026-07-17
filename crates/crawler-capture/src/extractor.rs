//! Static article-body extraction and image candidate discovery.
//!
//! Rich text is parsed as a DOM and rendered to a deliberately small safe
//! subset. This preserves article semantics without carrying scripts, styles,
//! event handlers, or unsafe URL schemes into later storage layers.

use ego_tree::NodeRef;
use scraper::{ElementRef, Html, Node, Selector};

use crate::error::{CrawlerError, CrawlerResult};
use crate::types::ImageCandidate;

#[derive(Debug, Clone)]
pub struct ExtractedBody {
    pub title: String,
    pub author: Option<String>,
    /// Sanitized rich HTML. Original fetched HTML is retained separately by the
    /// capture layer when raw-object retention is enabled.
    pub content_html: String,
    /// Semantic capture Markdown. Remote image URLs are subsequently replaced
    /// with controlled asset references by the service layer.
    pub content_md: String,
    pub candidates: Vec<ImageCandidate>,
    pub text_length: u32,
    /// True when a lazy image has no usable static source.
    pub needs_lazy_render: bool,
}

pub struct BodyExtractor;

impl BodyExtractor {
    pub fn new() -> Self {
        Self
    }

    pub fn extract(&self, html: &str, page_url: &str) -> CrawlerResult<ExtractedBody> {
        let document = Html::parse_document(html);
        let title = extract_title(&document).unwrap_or_default();
        let author = extract_author(&document);
        let raw_content_html = extract_content_html(&document);
        if raw_content_html.trim().is_empty() {
            return Err(CrawlerError::empty_article("article body DOM is empty"));
        }

        let rendered = sanitize_and_render(&raw_content_html, page_url);
        let content_html = rendered.html;
        let content_md = rendered.markdown;
        let text_length = text_length(&content_html);
        if text_length == 0 && !content_html.contains("<img") {
            return Err(CrawlerError::empty_article(
                "article has neither text nor images",
            ));
        }

        // Candidate discovery intentionally uses the original body DOM. Hidden
        // images can then be recorded as decisions, while advertisement subtrees
        // and unsafe URLs are rejected before any downloader sees them.
        let (candidates, needs_lazy) = extract_candidates(&document, &raw_content_html, page_url);

        Ok(ExtractedBody {
            title,
            author,
            content_html,
            content_md,
            candidates,
            text_length,
            needs_lazy_render: needs_lazy,
        })
    }
}

impl Default for BodyExtractor {
    fn default() -> Self {
        Self::new()
    }
}

fn extract_title(document: &Html) -> Option<String> {
    for sel in ["h1.rich_media_title", "h1#activity-name", "title", "h1"] {
        if let Ok(selector) = Selector::parse(sel)
            && let Some(el) = document.select(&selector).next()
        {
            let text = normalize_text(&el.text().collect::<String>());
            if !text.is_empty() {
                return Some(text);
            }
        }
    }
    None
}

fn extract_author(document: &Html) -> Option<String> {
    for sel in [
        "a#js_name",
        "span.rich_media_meta_text",
        "meta[name=author]",
    ] {
        if let Ok(selector) = Selector::parse(sel)
            && let Some(el) = document.select(&selector).next()
        {
            let text = if sel.starts_with("meta") {
                el.value().attr("content").map(normalize_text)
            } else {
                Some(normalize_text(&el.text().collect::<String>()))
            };
            if let Some(text) = text.filter(|value| !value.is_empty()) {
                return Some(text);
            }
        }
    }
    None
}

fn extract_content_html(document: &Html) -> String {
    for selector in [
        "div#js_content",
        "div.rich_media_content",
        "article",
        "main",
        "body",
    ] {
        if let Ok(selector) = Selector::parse(selector)
            && let Some(el) = document.select(&selector).next()
        {
            let html = el.html();
            if !html.trim().is_empty() {
                return html;
            }
        }
    }
    String::new()
}

#[derive(Default)]
struct Rendered {
    html: String,
    markdown: String,
}

impl Rendered {
    fn join(mut self, other: Self) -> Self {
        self.html.push_str(&other.html);
        self.markdown.push_str(&other.markdown);
        self
    }

    fn into_block(mut self) -> Self {
        if !self.markdown.trim().is_empty() {
            self.markdown = format!("{}\n\n", self.markdown.trim());
        }
        self
    }
}

fn sanitize_and_render(html: &str, page_url: &str) -> Rendered {
    let document = Html::parse_fragment(html);
    let rendered = render_children(&document.root_element(), page_url);
    Rendered {
        html: rendered.html.trim().to_string(),
        markdown: normalize_markdown(&rendered.markdown),
    }
}

fn render_children(node: &NodeRef<'_, Node>, page_url: &str) -> Rendered {
    node.children().fold(Rendered::default(), |out, child| {
        out.join(render_node(&child, page_url))
    })
}

fn render_node(node: &NodeRef<'_, Node>, page_url: &str) -> Rendered {
    match node.value() {
        Node::Text(text) => {
            // Preserve a single meaningful boundary space between adjacent inline
            // nodes (`Heading <em>one</em>`, `Text <a>link</a>`), while still
            // collapsing layout indentation from the source DOM.
            let text = normalize_inline_text(text);
            Rendered {
                html: escape_html(&text),
                markdown: escape_markdown(&text),
            }
        }
        Node::Element(_) => {
            let Some(el) = ElementRef::wrap(*node) else {
                return Rendered::default();
            };
            if should_drop_element(el.value()) {
                return Rendered::default();
            }
            render_element(&el, &el.value().name().to_ascii_lowercase(), page_url)
        }
        _ => Rendered::default(),
    }
}

fn render_element(el: &ElementRef<'_>, tag: &str, page_url: &str) -> Rendered {
    match tag {
        "br" => Rendered {
            html: "<br />".into(),
            markdown: "  \n".into(),
        },
        "hr" => Rendered {
            html: "<hr />".into(),
            markdown: "---\n\n".into(),
        },
        "img" => render_image(el, page_url),
        "audio" | "video" => render_media(el, tag, page_url),
        "ul" | "ol" => render_list(el, tag, page_url),
        "table" => render_table(el, page_url),
        "pre" => render_pre(el),
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
            let level = tag[1..].parse::<usize>().unwrap_or(1);
            let child = render_children(el, page_url);
            let text = child.markdown.trim().to_string();
            Rendered {
                html: wrap_html(tag, &child.html),
                markdown: if !text.is_empty() {
                    format!("{} {text}\n\n", "#".repeat(level))
                } else {
                    String::new()
                },
            }
        }
        "p" | "div" | "section" | "article" | "main" | "figure" | "figcaption" => {
            let child = render_children(el, page_url).into_block();
            Rendered {
                html: wrap_html(tag, &child.html),
                markdown: child.markdown,
            }
        }
        "blockquote" => {
            let child = render_children(el, page_url);
            let body = child.markdown.trim().to_string();
            Rendered {
                html: wrap_html("blockquote", &child.html),
                markdown: if !body.is_empty() {
                    format!("> {}\n\n", body.replace('\n', "\n> "))
                } else {
                    String::new()
                },
            }
        }
        "strong" | "b" => render_inline_wrapper(el, "strong", "**", page_url),
        "em" | "i" => render_inline_wrapper(el, "em", "*", page_url),
        "del" | "s" | "strike" => render_inline_wrapper(el, "del", "~~", page_url),
        "code" => {
            let child = render_children(el, page_url);
            let text = child.markdown.trim().to_string();
            Rendered {
                html: wrap_html("code", &child.html),
                markdown: if !text.is_empty() {
                    format!("`{}`", text.replace('`', "\\`"))
                } else {
                    String::new()
                },
            }
        }
        "a" => render_link(el, page_url),
        "li" => {
            let child = render_children(el, page_url).into_block();
            Rendered {
                html: wrap_html("li", &child.html),
                markdown: child.markdown,
            }
        }
        "span" | "small" | "sup" | "sub" | "mark" | "kbd" | "tbody" | "thead" | "tfoot" | "tr"
        | "th" | "td" => {
            let child = render_children(el, page_url);
            Rendered {
                html: wrap_html(tag, &child.html),
                markdown: child.markdown,
            }
        }
        // Unknown tags are flattened. No source attributes survive sanitization.
        _ => render_children(el, page_url),
    }
}

fn render_inline_wrapper(
    el: &ElementRef<'_>,
    safe_tag: &str,
    marker: &str,
    page_url: &str,
) -> Rendered {
    let child = render_children(el, page_url);
    let text = child.markdown.trim().to_string();
    Rendered {
        html: wrap_html(safe_tag, &child.html),
        markdown: if !text.is_empty() {
            format!("{marker}{text}{marker}")
        } else {
            String::new()
        },
    }
}

fn render_image(el: &ElementRef<'_>, page_url: &str) -> Rendered {
    if is_hidden(el.value()) {
        return Rendered::default();
    }
    let (url, _) = resolve_img_url(el.value());
    let Some(url) = url.and_then(|value| safe_asset_url(page_url, &value)) else {
        return Rendered::default();
    };
    let alt = el.attr("alt").map(normalize_text).unwrap_or_default();
    let html = if alt.is_empty() {
        format!(r#"<img src="{}" />"#, escape_attr(&url))
    } else {
        format!(
            r#"<img src="{}" alt="{}" />"#,
            escape_attr(&url),
            escape_attr(&alt)
        )
    };
    Rendered {
        html,
        markdown: format!("![{}]({url})\n\n", escape_markdown(&alt)),
    }
}

fn render_media(el: &ElementRef<'_>, tag: &str, page_url: &str) -> Rendered {
    let url = el
        .attr("src")
        .or_else(|| el.attr("data-src"))
        .or_else(|| {
            el.child_elements()
                .find(|child| child.value().name().eq_ignore_ascii_case("source"))
                .and_then(|source| source.attr("src"))
        })
        .and_then(|value| safe_asset_url(page_url, value));
    let label = if tag == "audio" { "Audio" } else { "Video" };
    let placeholder = match url {
        Some(url) => format!("[{label}: {url}]"),
        None => format!("[{label}]"),
    };
    Rendered {
        html: format!("<p>{}</p>", escape_html(&placeholder)),
        markdown: format!("{placeholder}\n\n"),
    }
}

fn render_link(el: &ElementRef<'_>, page_url: &str) -> Rendered {
    let child = render_children(el, page_url);
    let text = child.markdown.trim().to_string();
    match el
        .attr("href")
        .and_then(|value| safe_link_url(page_url, value))
    {
        Some(href) if !text.is_empty() => Rendered {
            html: format!(r#"<a href="{}">{}</a>"#, escape_attr(&href), child.html),
            markdown: format!("[{text}]({href})"),
        },
        _ => child,
    }
}

fn render_pre(el: &ElementRef<'_>) -> Rendered {
    let text = el.text().collect::<String>().replace("\r\n", "\n");
    let text = text.trim_matches('\n');
    if text.trim().is_empty() {
        return Rendered::default();
    }
    Rendered {
        html: format!("<pre><code>{}</code></pre>", escape_html(text)),
        markdown: format!("```\n{text}\n```\n\n"),
    }
}

fn render_list(el: &ElementRef<'_>, tag: &str, page_url: &str) -> Rendered {
    let mut html = String::new();
    let mut markdown = String::new();
    let mut number = 1usize;
    for li in el
        .child_elements()
        .filter(|child| child.value().name().eq_ignore_ascii_case("li"))
    {
        let child = render_children(&li, page_url);
        html.push_str(&wrap_html("li", &child.html));
        let body = child.markdown.trim().to_string();
        if !body.is_empty() {
            let prefix = if tag == "ol" {
                let prefix = format!("{number}. ");
                number += 1;
                prefix
            } else {
                "- ".to_string()
            };
            let continuation = " ".repeat(prefix.len());
            markdown.push_str(&prefix);
            markdown.push_str(&body.replace('\n', &format!("\n{continuation}")));
            markdown.push('\n');
        }
    }
    Rendered {
        html: wrap_html(tag, &html),
        markdown: if !markdown.is_empty() {
            format!("{}\n", markdown.trim_end())
        } else {
            String::new()
        },
    }
}

fn render_table(el: &ElementRef<'_>, page_url: &str) -> Rendered {
    let row_selector = Selector::parse("tr").expect("valid row selector");
    let cell_selector = Selector::parse("th, td").expect("valid cell selector");
    let rows = el
        .select(&row_selector)
        .map(|row| {
            row.select(&cell_selector)
                .map(|cell| normalize_text(&cell.text().collect::<String>()))
                .collect::<Vec<_>>()
        })
        .filter(|row| !row.is_empty())
        .collect::<Vec<_>>();
    let child = render_children(el, page_url);
    if rows.is_empty() {
        let child = child.into_block();
        return Rendered {
            html: wrap_html("table", &child.html),
            markdown: child.markdown,
        };
    }
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    let mut markdown = String::new();
    for (index, row) in rows.iter().enumerate() {
        let values = (0..columns)
            .map(|column| row.get(column).map(String::as_str).unwrap_or(""))
            .map(|cell| cell.replace('|', "\\|"))
            .collect::<Vec<_>>();
        markdown.push_str(&format!("| {} |\n", values.join(" | ")));
        if index == 0 {
            markdown.push_str(&format!("| {} |\n", vec!["---"; columns].join(" | ")));
        }
    }
    Rendered {
        html: wrap_html("table", &child.html),
        markdown: format!("{}\n", markdown.trim_end()),
    }
}

fn should_drop_element(el: &scraper::node::Element) -> bool {
    matches!(
        el.name().to_ascii_lowercase().as_str(),
        "script"
            | "style"
            | "noscript"
            | "template"
            | "iframe"
            | "object"
            | "embed"
            | "form"
            | "input"
            | "button"
            | "svg"
            | "canvas"
    ) || has_non_content_marker(el)
}

fn has_non_content_marker(el: &scraper::node::Element) -> bool {
    const MARKERS: &[&str] = &[
        "ad",
        "ads",
        "advert",
        "advertisement",
        "sponsor",
        "promotion",
        "promoted",
        "recommend",
        "reward",
        "qrcode",
        "qr",
        "toolbar",
    ];
    [el.attr("id"), el.attr("class")]
        .into_iter()
        .flatten()
        .any(|value| {
            let lower = value.to_ascii_lowercase();
            lower
                .split(|c: char| !c.is_ascii_alphanumeric())
                .any(|token| MARKERS.contains(&token))
        })
        || el
            .attr("aria-hidden")
            .is_some_and(|value| value.eq_ignore_ascii_case("true"))
}

fn is_hidden(el: &scraper::node::Element) -> bool {
    let style = el.attr("style").unwrap_or("").to_ascii_lowercase();
    el.attr("hidden").is_some()
        || style.contains("display:none")
        || style.contains("visibility:hidden")
        || el
            .attr("aria-hidden")
            .is_some_and(|value| value.eq_ignore_ascii_case("true"))
}

fn wrap_html(tag: &str, child_html: &str) -> String {
    format!("<{tag}>{child_html}</{tag}>")
}

fn normalize_text(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn normalize_inline_text(text: &str) -> String {
    let normalized = normalize_text(text);
    if normalized.is_empty() {
        return String::new();
    }
    let leading = text.chars().next().is_some_and(char::is_whitespace);
    let trailing = text.chars().last().is_some_and(char::is_whitespace);
    format!(
        "{}{}{}",
        if leading { " " } else { "" },
        normalized,
        if trailing { " " } else { "" }
    )
}

fn normalize_markdown(markdown: &str) -> String {
    let mut output = String::new();
    let mut blank_count = 0usize;
    for line in markdown.lines() {
        if line.trim().is_empty() {
            blank_count += 1;
            if blank_count <= 1 {
                output.push('\n');
            }
        } else {
            blank_count = 0;
            output.push_str(line.trim_end());
            output.push('\n');
        }
    }
    output.trim().to_string()
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn escape_attr(value: &str) -> String {
    escape_html(value)
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn escape_markdown(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('[', "\\[")
        .replace(']', "\\]")
        .replace('*', "\\*")
        .replace('_', "\\_")
}

fn safe_link_url(base: &str, value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() || value.starts_with('#') {
        return None;
    }
    if value.to_ascii_lowercase().starts_with("mailto:") {
        return Some(value.to_string());
    }
    safe_http_url(base, value)
}

fn safe_asset_url(base: &str, value: &str) -> Option<String> {
    safe_http_url(base, value)
}

fn safe_http_url(base: &str, value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    let url = url::Url::parse(value)
        .or_else(|_| url::Url::parse(base).and_then(|base| base.join(value)))
        .ok()?;
    matches!(url.scheme(), "http" | "https").then(|| url.to_string())
}

fn text_length(html: &str) -> u32 {
    let doc = Html::parse_fragment(html);
    let text: String = doc.root_element().text().collect();
    text.trim().chars().count() as u32
}

fn extract_candidates(
    document: &Html,
    content_html: &str,
    page_url: &str,
) -> (Vec<ImageCandidate>, bool) {
    let body_doc = Html::parse_fragment(content_html);
    let Ok(img_sel) = Selector::parse("img") else {
        return (vec![], false);
    };

    let mut body_urls = std::collections::HashSet::new();
    let mut needs_lazy = false;
    let mut candidates = Vec::new();

    for el in body_doc.select(&img_sel) {
        if is_in_dropped_subtree(el) {
            continue;
        }
        let (url, lazy) = resolve_img_url(el.value());
        if lazy && url.is_none() {
            needs_lazy = true;
        }
        let Some(url) = url.and_then(|url| safe_asset_url(page_url, &url)) else {
            continue;
        };
        body_urls.insert(url.clone());
        candidates.push(candidate_from_el(el.value(), url, true, lazy));
    }

    // Page-wide images remain decorative candidates only; they can never gain
    // `in_body` merely because they share a URL with an article image.
    for el in document.select(&img_sel) {
        if is_in_dropped_subtree(el) {
            continue;
        }
        let (url, lazy) = resolve_img_url(el.value());
        if lazy && url.is_none() {
            needs_lazy = true;
        }
        let Some(url) = url.and_then(|url| safe_asset_url(page_url, &url)) else {
            continue;
        };
        if body_urls.contains(&url) {
            continue;
        }
        candidates.push(candidate_from_el(el.value(), url, false, lazy));
    }

    (candidates, needs_lazy)
}

fn is_in_dropped_subtree(el: ElementRef<'_>) -> bool {
    el.ancestors()
        .filter_map(ElementRef::wrap)
        .any(|ancestor| should_drop_element(ancestor.value()))
}

fn resolve_img_url(el: &scraper::node::Element) -> (Option<String>, bool) {
    let lazy_attrs = [
        "data-src",
        "data-original",
        "data-lazy-src",
        "data-actualsrc",
    ];
    for attr in lazy_attrs {
        if let Some(value) = el.attr(attr) {
            let value = value.trim();
            if !value.is_empty() {
                return (Some(value.to_string()), true);
            }
        }
    }
    if let Some(value) = el.attr("src") {
        let value = value.trim();
        if !value.is_empty() && !value.starts_with("data:") {
            return (Some(value.to_string()), false);
        }
        if value.starts_with("data:") {
            return (None, true);
        }
    }
    for attr in lazy_attrs {
        if el.attr(attr).is_some() {
            return (None, true);
        }
    }
    (None, false)
}

fn candidate_from_el(
    el: &scraper::node::Element,
    url: String,
    in_body: bool,
    lazy: bool,
) -> ImageCandidate {
    let class = el.attr("class").unwrap_or("").to_ascii_lowercase();
    let style = el.attr("style").unwrap_or("").to_ascii_lowercase();
    let hidden = el.attr("hidden").is_some()
        || style.contains("display:none")
        || style.contains("visibility:hidden")
        || class.contains("hidden");
    let decorative_hint = !in_body
        || class.contains("emoji")
        || class.contains("icon")
        || class.contains("qrcode")
        || class.contains("reward")
        || url.contains("emoji")
        || url.contains("/icon");

    ImageCandidate {
        url,
        alt: el.attr("alt").map(str::to_string),
        in_body,
        hidden,
        lazy,
        decorative_hint,
        declared_width: parse_dim(el.attr("width")),
        declared_height: parse_dim(el.attr("height")),
    }
}

fn parse_dim(value: Option<&str>) -> Option<u32> {
    value?.trim().trim_end_matches("px").parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"<!doctype html>
<html><head><title>Fixture Title</title><meta name="author" content="Author A" /></head>
<body>
<div class="header"><img class="icon" src="https://cdn.example.com/icon/logo.png" /></div>
<div id="js_content" style="color:red" onclick="evil()">
  <h2>Heading <em>one</em></h2>
  <p>A <strong>paragraph</strong> with <a href="/read">a link</a> and <code>x()</code>.</p>
  <ul><li>first</li><li>second</li></ul>
  <blockquote>quoted text</blockquote>
  <pre><code>let x = 1;</code></pre>
  <table><tr><th>Name</th><th>Value</th></tr><tr><td>A</td><td>1</td></tr></table>
  <p>Before <img data-src="/body/lazy.jpg" alt="hero" width="400" height="300" /> after.</p>
  <audio data-src="/media/a.mp3"></audio><video src="javascript:bad"></video>
  <img style="display:none" src="https://cdn.example.com/track.gif" width="1" height="1" />
  <div class="advertisement"><script>alert(1)</script><p>ad copy</p><img src="/ad.png" /></div>
  <p><a href="javascript:alert(1)">unsafe</a></p>
</div></body></html>"#;

    #[test]
    fn fixture_preserves_semantics_and_sanitizes_rich_html() {
        let extracted = BodyExtractor::new()
            .extract(FIXTURE, "https://mp.weixin.qq.com/s/fixture")
            .unwrap();

        assert_eq!(extracted.title, "Fixture Title");
        assert_eq!(extracted.author.as_deref(), Some("Author A"));
        assert!(extracted.content_md.contains("## Heading *one*"));
        assert!(extracted.content_md.contains("**paragraph**"));
        assert!(
            extracted
                .content_md
                .contains("[a link](https://mp.weixin.qq.com/read)")
        );
        assert!(extracted.content_md.contains("- first"));
        assert!(extracted.content_md.contains("> quoted text"));
        assert!(extracted.content_md.contains("```\nlet x = 1;\n```"));
        assert!(extracted.content_md.contains("| Name | Value |"));
        assert!(
            extracted
                .content_md
                .contains("![hero](https://mp.weixin.qq.com/body/lazy.jpg)")
        );
        assert!(
            extracted
                .content_md
                .contains("[Audio: https://mp.weixin.qq.com/media/a.mp3]")
        );
        assert!(extracted.content_md.contains("[Video]"));
        assert!(!extracted.content_md.contains("ad copy"));
        assert!(!extracted.content_md.contains("javascript:"));
        assert!(!extracted.content_html.contains("style="));
        assert!(!extracted.content_html.contains("onclick"));
        assert!(!extracted.content_html.contains("script"));
        assert!(!extracted.content_html.contains("advertisement"));
        assert!(
            extracted
                .content_html
                .contains("<a href=\"https://mp.weixin.qq.com/read\">")
        );
    }

    #[test]
    fn fixture_keeps_body_candidate_positions_and_filters_unsafe_or_advertising_candidates() {
        let extracted = BodyExtractor::new()
            .extract(FIXTURE, "https://mp.weixin.qq.com/s/fixture")
            .unwrap();
        assert!(extracted.candidates.iter().any(|candidate| {
            candidate.url.ends_with("/body/lazy.jpg") && candidate.in_body && candidate.lazy
        }));
        assert!(
            extracted
                .candidates
                .iter()
                .any(|candidate| candidate.url.contains("track.gif") && candidate.hidden)
        );
        assert!(
            !extracted
                .candidates
                .iter()
                .any(|candidate| candidate.url.ends_with("/ad.png"))
        );
        assert!(
            !extracted
                .candidates
                .iter()
                .any(|candidate| candidate.url.starts_with("javascript:"))
        );
    }

    #[test]
    fn unsafe_links_and_data_images_are_not_rendered() {
        let html = r#"<div id="js_content"><p>Text <a href="data:text/html,boom">bad</a></p><img src="data:image/png;base64,a" /></div>"#;
        let extracted = BodyExtractor::new()
            .extract(html, "https://mp.weixin.qq.com/s/fixture")
            .unwrap();
        assert_eq!(extracted.content_md, "Text bad");
        assert!(!extracted.content_html.contains("href="));
        assert!(!extracted.content_html.contains("img"));
    }
}
