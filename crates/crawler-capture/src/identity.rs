//! ArticleIdentityResolver：优先 source+biz+mid+idx，否则 canonical URL。

use serde::{Deserialize, Serialize};
use url::{Url, form_urlencoded};
use uuid::Uuid;

/// 解析后的文章身份（用于幂等键与去重）。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ArticleIdentity {
    pub source_account_id: Uuid,
    pub kind: ArticleIdentityKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArticleIdentityKind {
    Wechat {
        biz: String,
        mid: String,
        idx: String,
    },
    CanonicalUrl {
        url: String,
    },
}

impl ArticleIdentity {
    /// 稳定幂等键片段（不含 `detail:` 前缀）。
    pub fn key(&self) -> String {
        match &self.kind {
            ArticleIdentityKind::Wechat { biz, mid, idx } => {
                format!("{}:{}:{}:{}", self.source_account_id, biz, mid, idx)
            }
            ArticleIdentityKind::CanonicalUrl { url } => {
                format!("{}:url:{}", self.source_account_id, url)
            }
        }
    }

    pub fn wechat_parts(&self) -> Option<(&str, &str, &str)> {
        match &self.kind {
            ArticleIdentityKind::Wechat { biz, mid, idx } => Some((biz, mid, idx)),
            ArticleIdentityKind::CanonicalUrl { .. } => None,
        }
    }

    pub fn canonical_url_hint(&self) -> Option<&str> {
        match &self.kind {
            ArticleIdentityKind::CanonicalUrl { url } => Some(url.as_str()),
            ArticleIdentityKind::Wechat { .. } => None,
        }
    }
}

/// 从 URL / 显式字段解析文章身份。
#[derive(Debug, Default, Clone)]
pub struct ArticleIdentityResolver;

#[derive(Debug, Clone, Default)]
pub struct IdentityInput<'a> {
    pub source_account_id: Uuid,
    pub canonical_url: &'a str,
    pub wechat_biz: Option<&'a str>,
    pub wechat_mid: Option<&'a str>,
    pub wechat_idx: Option<&'a str>,
}

impl ArticleIdentityResolver {
    pub fn new() -> Self {
        Self
    }

    pub fn resolve(&self, input: IdentityInput<'_>) -> ArticleIdentity {
        let biz = nonempty(input.wechat_biz).or_else(|| query_param(input.canonical_url, "__biz"));
        let mid = nonempty(input.wechat_mid).or_else(|| query_param(input.canonical_url, "mid"));
        let idx = nonempty(input.wechat_idx).or_else(|| query_param(input.canonical_url, "idx"));

        let kind = match (biz, mid, idx) {
            (Some(biz), Some(mid), Some(idx)) => ArticleIdentityKind::Wechat { biz, mid, idx },
            _ => ArticleIdentityKind::CanonicalUrl {
                url: canonicalize_url(input.canonical_url),
            },
        };

        ArticleIdentity {
            source_account_id: input.source_account_id,
            kind,
        }
    }
}

fn nonempty(v: Option<&str>) -> Option<String> {
    v.map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn query_param(raw_url: &str, key: &str) -> Option<String> {
    let url = Url::parse(raw_url).ok()?;
    url.query_pairs()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.into_owned())
        .filter(|v| !v.is_empty())
}

/// 去掉 fragment；保留 path/query；小写 host。
pub fn canonicalize_url(raw: &str) -> String {
    let Ok(mut url) = Url::parse(raw.trim()) else {
        return raw.trim().to_string();
    };
    url.set_fragment(None);
    let _ = url.set_username("");
    let _ = url.set_password(None);
    if let Some(host) = url.host_str().map(|h| h.to_ascii_lowercase()) {
        let _ = url.set_host(Some(&host));
    }

    let mut pairs: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(key, _)| !is_sensitive_query_key(key.as_ref()))
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    pairs.sort_unstable();
    if pairs.is_empty() {
        url.set_query(None);
    } else {
        let query = form_urlencoded::Serializer::new(String::new())
            .extend_pairs(
                pairs
                    .iter()
                    .map(|(key, value)| (key.as_str(), value.as_str())),
            )
            .finish();
        url.set_query(Some(&query));
    }
    url.to_string()
}

/// Login/session capabilities must not be retained in canonical URLs.
fn is_sensitive_query_key(key: &str) -> bool {
    matches!(
        key.to_ascii_lowercase().as_str(),
        "uin"
            | "key"
            | "pass_ticket"
            | "appmsg_token"
            | "token"
            | "cookie"
            | "access_token"
            | "refresh_token"
            | "authorization"
            | "login_ticket"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefers_biz_mid_idx_over_url() {
        let source = Uuid::nil();
        let id = ArticleIdentityResolver.resolve(IdentityInput {
            source_account_id: source,
            canonical_url: "https://mp.weixin.qq.com/s/other",
            wechat_biz: Some("biz1"),
            wechat_mid: Some("100"),
            wechat_idx: Some("1"),
        });
        assert_eq!(
            id.kind,
            ArticleIdentityKind::Wechat {
                biz: "biz1".into(),
                mid: "100".into(),
                idx: "1".into(),
            }
        );
        assert_eq!(id.key(), format!("{source}:biz1:100:1"));
    }

    #[test]
    fn extracts_from_query_string() {
        let source = Uuid::new_v4();
        let url = "https://mp.weixin.qq.com/s?__biz=MzA1&mid=2247&idx=2&sn=abc#rd";
        let id = ArticleIdentityResolver.resolve(IdentityInput {
            source_account_id: source,
            canonical_url: url,
            wechat_biz: None,
            wechat_mid: None,
            wechat_idx: None,
        });
        assert!(matches!(
            id.kind,
            ArticleIdentityKind::Wechat {
                ref biz,
                ref mid,
                ref idx
            } if biz == "MzA1" && mid == "2247" && idx == "2"
        ));
    }

    #[test]
    fn falls_back_to_canonical_url() {
        let source = Uuid::new_v4();
        let id = ArticleIdentityResolver.resolve(IdentityInput {
            source_account_id: source,
            canonical_url: "https://Example.com/a/b?x=1#frag",
            wechat_biz: None,
            wechat_mid: None,
            wechat_idx: None,
        });
        match id.kind {
            ArticleIdentityKind::CanonicalUrl { url } => {
                assert!(url.starts_with("https://example.com/a/b"));
                assert!(!url.contains('#'));
            }
            _ => panic!("expected canonical url"),
        }
    }

    #[test]
    fn same_wechat_identity_dedups() {
        let source = Uuid::new_v4();
        let a = ArticleIdentityResolver.resolve(IdentityInput {
            source_account_id: source,
            canonical_url: "https://mp.weixin.qq.com/s/a",
            wechat_biz: Some("b"),
            wechat_mid: Some("1"),
            wechat_idx: Some("1"),
        });
        let b = ArticleIdentityResolver.resolve(IdentityInput {
            source_account_id: source,
            canonical_url: "https://mp.weixin.qq.com/s/b",
            wechat_biz: Some("b"),
            wechat_mid: Some("1"),
            wechat_idx: Some("1"),
        });
        assert_eq!(a.key(), b.key());
    }

    #[test]
    fn canonical_url_sanitizes_secrets_and_stabilizes_query_order() {
        let canonical = canonicalize_url(
            "https://user:secret@MP.WEIXIN.QQ.COM/s?z=2&uin=1&__biz=MzA1&key=secret&idx=2&mid=9&appmsg_token=x#rd",
        );
        assert_eq!(
            canonical,
            "https://mp.weixin.qq.com/s?__biz=MzA1&idx=2&mid=9&z=2"
        );
        assert!(!canonical.contains("secret"));
        assert!(!canonical.contains("uin="));
    }

    #[test]
    fn canonical_url_keeps_article_signature_but_drops_session_tokens() {
        let canonical = canonicalize_url(
            "https://mp.weixin.qq.com/s?sn=article-signature&pass_ticket=secret&AppMsg_Token=secret",
        );
        assert_eq!(canonical, "https://mp.weixin.qq.com/s?sn=article-signature");
    }
}
