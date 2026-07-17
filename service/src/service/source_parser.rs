//! SourceParser：从微信文章链接提取 identity 与公众号强标识。

use crawler_capture::canonicalize_url;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use url::Url;

/// 解析后的监测公众号样例链接信息。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParsedSource {
    pub input_url: String,
    pub canonical_url: String,
    pub wechat_biz: Option<String>,
    pub wechat_mid: Option<String>,
    pub wechat_idx: Option<String>,
    pub wechat_sn: Option<String>,
    pub title_hint: Option<String>,
    /// 强标识值（通常为 biz）；缺失时无法做强标识去重。
    pub strong_id_type: Option<String>,
    pub strong_id_value: Option<String>,
    pub display_name_hint: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct SourceParser;

impl SourceParser {
    pub fn new() -> Self {
        Self
    }

    /// 解析文章 URL；要求至少能得到 canonical URL。
    pub fn parse(&self, raw_url: &str) -> Result<ParsedSource, String> {
        let trimmed = raw_url.trim();
        if trimmed.is_empty() {
            return Err("source.invalid_url: URL 不能为空".into());
        }
        let mut parsed = Url::parse(trimmed).map_err(|e| format!("source.invalid_url: {e}"))?;
        if parsed.scheme() != "http" && parsed.scheme() != "https" {
            return Err("source.invalid_url: 仅支持 http/https".into());
        }
        let host = parsed
            .host_str()
            .map(|value| value.trim_end_matches('.').to_ascii_lowercase())
            .ok_or_else(|| "source.invalid_url: URL 缺少 host".to_string())?;
        if host != "mp.weixin.qq.com" {
            return Err("source.invalid_url: 仅接受 mp.weixin.qq.com 文章链接".into());
        }
        if parsed.path() != "/s" && !parsed.path().starts_with("/s/") {
            return Err("source.invalid_url: 仅接受微信公众号文章路径".into());
        }
        if let Some(port) = parsed.port()
            && !matches!((parsed.scheme(), port), ("http", 80) | ("https", 443))
        {
            return Err("source.invalid_url: 不接受非标准端口".into());
        }

        // `input_url` is persisted in source_validation_runs. Normalize before
        // creating ParsedSource so pasted session credentials never reach the
        // database, job payload or safe context.
        parsed
            .set_scheme("https")
            .map_err(|_| "source.invalid_url: URL scheme 无法规范化".to_string())?;
        let _ = parsed.set_host(Some("mp.weixin.qq.com"));
        let _ = parsed.set_port(None);
        let _ = parsed.set_username("");
        let _ = parsed.set_password(None);
        parsed.set_fragment(None);

        let canonical = canonicalize_url(parsed.as_str());
        let canonical_parsed =
            Url::parse(&canonical).map_err(|_| "source.invalid_url: URL 无法规范化".to_string())?;
        let (biz, mid, idx, sn) = extract_wechat_params(&canonical_parsed);
        let strong = biz.clone().map(|v| ("biz".to_string(), v));

        Ok(ParsedSource {
            input_url: canonical.clone(),
            canonical_url: canonical,
            wechat_biz: biz.clone(),
            wechat_mid: mid,
            wechat_idx: idx,
            wechat_sn: sn,
            title_hint: None,
            strong_id_type: strong.as_ref().map(|(t, _)| t.clone()),
            strong_id_value: strong.map(|(_, v)| v),
            display_name_hint: biz.map(|b| format!("biz:{b}")),
        })
    }

    /// Parse a redirect target first, then use only the already-fetched trusted
    /// WeChat HTML to fill a missing `__biz` identity.
    pub fn parse_resolved(&self, effective_url: &str, html: &str) -> Result<ParsedSource, String> {
        let mut parsed = self.parse(effective_url)?;
        if parsed.wechat_biz.is_some() {
            return Ok(parsed);
        }
        let Some(biz) = extract_biz_from_trusted_html(html) else {
            return Ok(parsed);
        };
        let mut url = Url::parse(&parsed.canonical_url)
            .map_err(|_| "source.invalid_url: URL cannot be normalized".to_string())?;
        let mut pairs: Vec<(String, String)> = url
            .query_pairs()
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect();
        pairs.push(("__biz".into(), biz));
        url.query_pairs_mut().clear().extend_pairs(pairs);
        parsed = self.parse(url.as_str())?;
        Ok(parsed)
    }

    /// 输入 URL 的稳定 hash（用于 validate 幂等）。
    pub fn input_hash(canonical_url: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(canonical_url.as_bytes());
        format!("{:x}", hasher.finalize())
    }
}

fn extract_biz_from_trusted_html(html: &str) -> Option<String> {
    for marker in ["__biz=", "__biz%3D"] {
        let Some(start) = html.find(marker).map(|index| index + marker.len()) else {
            continue;
        };
        let encoded: String = html[start..]
            .chars()
            .take_while(|ch| !matches!(ch, '&' | '"' | '\'' | '<' | '>' | '#' | ' ' | '\r' | '\n'))
            .take(256)
            .collect();
        let candidate = if marker.ends_with("%3D") {
            url::form_urlencoded::parse(format!("v={encoded}").as_bytes())
                .find_map(|(key, value)| (key == "v").then(|| value.into_owned()))
        } else {
            Some(encoded)
        }?;
        let candidate = candidate.trim_end_matches("amp;");
        if is_safe_biz(candidate) {
            return Some(candidate.to_string());
        }
    }
    None
}

fn is_safe_biz(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'='))
}

fn extract_wechat_params(
    url: &Url,
) -> (
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
) {
    let mut biz = None;
    let mut mid = None;
    let mut idx = None;
    let mut sn = None;
    for (k, v) in url.query_pairs() {
        let value = v.into_owned();
        if value.is_empty() {
            continue;
        }
        match k.as_ref() {
            "__biz" => biz = Some(value),
            "mid" => mid = Some(value),
            "idx" => idx = Some(value),
            "sn" => sn = Some(value),
            _ => {}
        }
    }
    (biz, mid, idx, sn)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_biz_mid_idx_sn() {
        let p = SourceParser::new()
            .parse("https://mp.weixin.qq.com/s?__biz=MzA1&mid=2247&idx=2&sn=abc#rd")
            .unwrap();
        assert_eq!(p.wechat_biz.as_deref(), Some("MzA1"));
        assert_eq!(p.wechat_mid.as_deref(), Some("2247"));
        assert_eq!(p.wechat_idx.as_deref(), Some("2"));
        assert_eq!(p.wechat_sn.as_deref(), Some("abc"));
        assert!(!p.canonical_url.contains('#'));
        assert_eq!(p.strong_id_type.as_deref(), Some("biz"));
        assert_eq!(p.strong_id_value.as_deref(), Some("MzA1"));
    }

    #[test]
    fn input_hash_stable() {
        let a = SourceParser::input_hash("https://mp.weixin.qq.com/s?__biz=x");
        let b = SourceParser::input_hash("https://mp.weixin.qq.com/s?__biz=x");
        assert_eq!(a, b);
        assert_eq!(a.len(), 64);
    }

    #[test]
    fn accepts_only_wechat_article_hosts_and_paths() {
        let parser = SourceParser::new();
        for raw in [
            "https://mp.weixin.qq.com.evil.test/s?__biz=x",
            "https://mp.weixin.qq.com@evil.test/s?__biz=x",
            "https://example.test/s?__biz=x",
            "https://mp.weixin.qq.com/mp/profile_ext?action=home&__biz=x",
            "https://mp.weixin.qq.com:444/s?__biz=x",
        ] {
            assert!(parser.parse(raw).is_err(), "must reject {raw}");
        }
        assert!(
            parser
                .parse("https://mp.weixin.qq.com/s/article?__biz=x")
                .is_ok()
        );
    }

    #[test]
    fn strips_sensitive_query_values_before_any_persistence() {
        let parsed = SourceParser::new()
            .parse(
                "http://user:secret@MP.WEIXIN.QQ.COM:80/s?z=2&appmsg_token=a&idx=2&mid=9&__biz=MzA1&uin=1&key=secret&pass_ticket=secret&sn=article#rd",
            )
            .unwrap();
        assert_eq!(
            parsed.canonical_url,
            "https://mp.weixin.qq.com/s?__biz=MzA1&idx=2&mid=9&sn=article&z=2"
        );
        assert_eq!(parsed.input_url, parsed.canonical_url);
        assert!(!serde_json::to_string(&parsed).unwrap().contains("secret"));
        assert_eq!(parsed.wechat_biz.as_deref(), Some("MzA1"));
    }

    #[test]
    fn canonicalization_makes_equivalent_query_orders_idempotent() {
        let parser = SourceParser::new();
        let a = parser
            .parse("https://mp.weixin.qq.com/s?idx=2&mid=9&__biz=MzA1&sn=article")
            .unwrap();
        let b = parser
            .parse("https://mp.weixin.qq.com/s?sn=article&__biz=MzA1&mid=9&idx=2")
            .unwrap();
        assert_eq!(a.canonical_url, b.canonical_url);
        assert_eq!(
            SourceParser::input_hash(&a.canonical_url),
            SourceParser::input_hash(&b.canonical_url)
        );
    }

    #[test]
    fn resolved_wechat_html_can_fill_missing_biz() {
        let parsed = SourceParser::new()
            .parse_resolved(
                "https://mp.weixin.qq.com/s/article?mid=9&idx=1",
                r#"<a href="/s?__biz=MzA1_safe==&amp;mid=9">article</a>"#,
            )
            .unwrap();
        assert_eq!(parsed.wechat_biz.as_deref(), Some("MzA1_safe=="));
        assert_eq!(parsed.strong_id_value.as_deref(), Some("MzA1_safe=="));
    }
}
