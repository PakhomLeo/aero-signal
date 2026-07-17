//! 公众号后台（mp.weixin.qq.com）HTTP 协议：扫码登录 + 已发消息列表。

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use chrono::{TimeZone, Utc};
use reqwest::header::{ACCEPT, ACCEPT_LANGUAGE, COOKIE, HeaderMap, HeaderValue, USER_AGENT};
use reqwest::{Client, Proxy, Response};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::error::{ConnectorError, ConnectorResult};
use crate::types::{ArticleCandidate, ArticleCandidateEvidence, NAME_MP_ADMIN};

pub const BASE_URL: &str = "https://mp.weixin.qq.com";

/// The protocol accepts at most ten records per `appmsgpublish` request.
const MP_ADMIN_PAGE_SIZE: usize = 10;
/// Incremental discovery is intentionally bounded. Historical recovery needs a separate,
/// checkpointed job in the service layer rather than an unbounded connector request.
const MAX_INCREMENTAL_PAGES: usize = 3;

#[derive(Debug)]
struct ParsedAppmsgPage {
    candidates: Vec<ArticleCandidate>,
    can_continue: bool,
}

#[cfg(test)]
static TEST_BASE_URL: std::sync::OnceLock<std::sync::Mutex<Option<String>>> =
    std::sync::OnceLock::new();

fn base_url() -> String {
    #[cfg(test)]
    if let Some(value) = TEST_BASE_URL
        .get_or_init(|| std::sync::Mutex::new(None))
        .lock()
        .expect("test base URL lock poisoned")
        .clone()
    {
        return value;
    }
    BASE_URL.to_string()
}

/// 持久化凭据（写入 SecretStore）。
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MpAdminCredentials {
    pub token: String,
    pub cookies: HashMap<String, String>,
    #[serde(default)]
    pub fakeid: Option<String>,
    #[serde(default)]
    pub nickname: Option<String>,
    #[serde(default)]
    pub authorized_mp_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy_url: Option<String>,
}

/// QR 会话中间态（ticket 内存 map）。
#[derive(Clone, Serialize, Deserialize)]
pub struct MpAdminQrSession {
    pub session_id: String,
    pub cookies: HashMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy_url: Option<String>,
}

fn default_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        USER_AGENT,
        HeaderValue::from_static(
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
             (KHTML, like Gecko) Chrome/123.0.0.0 Safari/537.36",
        ),
    );
    headers.insert(
        ACCEPT,
        HeaderValue::from_static(
            "text/html,application/xhtml+xml,application/xml;q=0.9,image/webp,*/*;q=0.8",
        ),
    );
    headers.insert(
        ACCEPT_LANGUAGE,
        HeaderValue::from_static("zh-CN,zh;q=0.9,en;q=0.8"),
    );
    headers
}

fn http_client(proxy_url: Option<&str>) -> ConnectorResult<Client> {
    let mut builder = Client::builder()
        .timeout(Duration::from_secs(20))
        .default_headers(default_headers())
        .redirect(reqwest::redirect::Policy::limited(10));
    if let Some(proxy_url) = proxy_url {
        builder = builder.proxy(
            Proxy::all(proxy_url).map_err(|_| ConnectorError::failed("固定出口代理配置无效"))?,
        );
    }
    builder
        .build()
        .map_err(|_| ConnectorError::failed("创建 mp_admin HTTP 客户端失败"))
}

fn cookie_header(cookies: &HashMap<String, String>) -> String {
    cookies
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("; ")
}

fn with_cookies(
    builder: reqwest::RequestBuilder,
    cookies: &HashMap<String, String>,
) -> reqwest::RequestBuilder {
    let header = cookie_header(cookies);
    if header.is_empty() {
        builder
    } else {
        builder.header(COOKIE, header)
    }
}

fn merge_cookies(
    existing: &HashMap<String, String>,
    response: &Response,
) -> HashMap<String, String> {
    let mut out = existing.clone();
    for cookie in response.cookies() {
        out.insert(cookie.name().to_string(), cookie.value().to_string());
    }
    out
}

pub fn env_protocol_enabled() -> bool {
    matches!(
        std::env::var("SIGNAL_MP_ADMIN_PROTOCOL").as_deref(),
        Ok("1") | Ok("true") | Ok("TRUE") | Ok("yes")
    )
}

pub async fn start_qr_session(
    proxy_url: Option<String>,
) -> ConnectorResult<(String, MpAdminQrSession)> {
    let client = http_client(proxy_url.as_deref())?;
    let session_id = format!(
        "{}-{}",
        Utc::now().timestamp_millis(),
        &Uuid::new_v4().simple().to_string()[..8]
    );
    let start = with_cookies(
        client
            .post(format!("{}/cgi-bin/bizlogin", base_url()))
            .query(&[("action", "startlogin")])
            .form(&[
                ("userlang", "zh_CN"),
                ("redirect_url", ""),
                ("login_type", "3"),
                ("sessionid", session_id.as_str()),
                ("token", ""),
                ("lang", "zh_CN"),
                ("f", "json"),
                ("ajax", "1"),
            ]),
        &HashMap::new(),
    )
    .send()
    .await
    .map_err(|_| ConnectorError::failed("mp_admin startlogin 请求失败"))?;

    let cookies = merge_cookies(&HashMap::new(), &start);
    if start.status().is_success()
        && let Ok(payload) = start.json::<Value>().await
        && let Some(error) = base_resp_connector_error(&payload)
    {
        return Err(error);
    }

    let qr_url = format!(
        "{}/cgi-bin/scanloginqrcode?action=getqrcode&random={}",
        base_url(),
        Utc::now().timestamp_millis()
    );
    let qr_resp = with_cookies(
        client
            .get(&qr_url)
            .header("Referer", format!("{}/", base_url())),
        &cookies,
    )
    .send()
    .await
    .map_err(|_| ConnectorError::failed("mp_admin 获取二维码请求失败"))?;
    let cookies = merge_cookies(&cookies, &qr_resp);
    if !qr_resp.status().is_success() {
        return Err(ConnectorError::failed(format!(
            "mp_admin 二维码 HTTP {}",
            qr_resp.status()
        )));
    }
    let bytes = qr_resp
        .bytes()
        .await
        .map_err(|_| ConnectorError::failed("mp_admin 获取二维码响应失败"))?;
    let data_url = qr_bytes_to_data_url(&bytes);

    Ok((
        data_url,
        MpAdminQrSession {
            session_id,
            cookies,
            proxy_url,
        },
    ))
}

fn qr_bytes_to_data_url(bytes: &[u8]) -> String {
    format!("data:image/png;base64,{}", STANDARD.encode(bytes))
}

pub async fn poll_qr(session: &mut MpAdminQrSession) -> ConnectorResult<(QrAskStatus, String)> {
    let client = http_client(session.proxy_url.as_deref())?;
    let response = with_cookies(
        client
            .get(format!("{}/cgi-bin/scanloginqrcode", base_url()))
            .query(&[
                ("action", "ask"),
                ("token", ""),
                ("lang", "zh_CN"),
                ("f", "json"),
                ("ajax", "1"),
            ]),
        &session.cookies,
    )
    .send()
    .await
    .map_err(|_| ConnectorError::failed("mp_admin 轮询请求失败"))?;
    session.cookies = merge_cookies(&session.cookies, &response);
    let payload: Value = response
        .json()
        .await
        .map_err(|_| ConnectorError::failed("mp_admin 轮询响应格式无效"))?;
    let status = parse_ask_status(&payload);
    let message = match status {
        QrAskStatus::Waiting => "等待扫码".into(),
        QrAskStatus::Scanned => "已扫码，请在手机上确认".into(),
        QrAskStatus::Confirmed => "登录成功".into(),
        QrAskStatus::Expired => "二维码已过期".into(),
        QrAskStatus::Failed => payload
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("扫码失败")
            .into(),
    };
    Ok((status, message))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QrAskStatus {
    Waiting,
    Scanned,
    Confirmed,
    Expired,
    Failed,
}

fn parse_ask_status(payload: &Value) -> QrAskStatus {
    let status = payload.get("status").and_then(|v| {
        v.as_i64()
            .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
    });
    match status {
        Some(4) | Some(6) => QrAskStatus::Scanned,
        Some(1) => QrAskStatus::Confirmed,
        Some(2) | Some(5) | Some(-1) => QrAskStatus::Expired,
        Some(3) => QrAskStatus::Failed,
        _ => QrAskStatus::Waiting,
    }
}

pub async fn finalize_login(session: &MpAdminQrSession) -> ConnectorResult<MpAdminCredentials> {
    let client = http_client(session.proxy_url.as_deref())?;
    let response = with_cookies(
        client
            .post(format!("{}/cgi-bin/bizlogin", base_url()))
            .query(&[("action", "login")])
            .form(&[
                ("userlang", "zh_CN"),
                ("redirect_url", ""),
                ("cookie_forbidden", "0"),
                ("cookie_cleaned", "0"),
                ("plugin_used", "0"),
                ("login_type", "3"),
                ("token", ""),
                ("lang", "zh_CN"),
                ("f", "json"),
                ("ajax", "1"),
            ]),
        &session.cookies,
    )
    .send()
    .await
    .map_err(|_| ConnectorError::failed("mp_admin login 请求失败"))?;

    let mut cookies = merge_cookies(&session.cookies, &response);
    let payload: Value = response.json().await.unwrap_or(Value::Null);
    if let Some(error) = base_resp_connector_error(&payload) {
        return Err(error);
    }
    let token = cookies
        .get("token")
        .cloned()
        .or_else(|| extract_token_from_payload(&payload));
    let token = token.ok_or_else(|| {
        ConnectorError::failed("mp_admin 登录未返回 token，请确认账号有公众号后台权限")
    })?;

    let mut creds = MpAdminCredentials {
        token: token.clone(),
        cookies: cookies.clone(),
        fakeid: None,
        nickname: None,
        authorized_mp_ids: vec![],
        proxy_url: session.proxy_url.clone(),
    };
    if let Ok(profile) = discover_profile(&client, &token, &mut cookies).await {
        creds.fakeid = profile.fakeid.clone();
        creds.nickname = profile.nickname.clone();
        creds.cookies = cookies;
        if let Some(ref fid) = profile.fakeid {
            creds.authorized_mp_ids.push(fid.clone());
        }
    }
    Ok(creds)
}

fn base_resp_connector_error(payload: &Value) -> Option<ConnectorError> {
    let base = payload.get("base_resp")?;
    let ret = base.get("ret")?.as_i64()?;
    if ret == 0 {
        return None;
    }
    let raw = base
        .get("errmsg")
        .or_else(|| base.get("err_msg"))
        .and_then(|v| v.as_str())
        .unwrap_or("mp_admin API 错误");
    let normalized = raw.to_ascii_lowercase();
    if matches!(ret, 200003 | 200013)
        || raw.contains("频繁")
        || raw.contains("限制")
        || normalized.contains("rate limit")
    {
        return Some(ConnectorError::platform_restricted(
            "微信平台限制了当前操作",
        ));
    }
    if matches!(ret, -3 | -6 | 200002)
        || raw.contains("登录")
        || normalized.contains("login")
        || normalized.contains("token")
    {
        return Some(ConnectorError::needs_relogin("mp_admin 登录态已失效"));
    }
    Some(ConnectorError::failed("mp_admin API 返回失败"))
}

fn extract_token_from_payload(payload: &Value) -> Option<String> {
    let redirect = payload
        .get("redirect_url")
        .or_else(|| payload.get("redirectUrl"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    extract_token_from_url(redirect)
}

fn extract_token_from_url(url: &str) -> Option<String> {
    if url.is_empty() {
        return None;
    }
    if let Ok(parsed) = url::Url::parse(url)
        && let Some(token) = parsed
            .query_pairs()
            .find(|(k, _)| k == "token")
            .map(|(_, v)| v.to_string())
        && !token.is_empty()
    {
        return Some(token);
    }
    regex::Regex::new(r#"token=([^&\s"']+)"#)
        .ok()?
        .captures(url)?
        .get(1)
        .map(|m| m.as_str().to_string())
}

#[derive(Debug, Clone, Default)]
struct HomeProfile {
    fakeid: Option<String>,
    nickname: Option<String>,
}

async fn discover_profile(
    client: &Client,
    token: &str,
    cookies: &mut HashMap<String, String>,
) -> ConnectorResult<HomeProfile> {
    let home = with_cookies(
        client
            .get(format!("{}/cgi-bin/home", base_url()))
            .query(&[("token", token), ("lang", "zh_CN")]),
        cookies,
    )
    .send()
    .await
    .map_err(|_| ConnectorError::failed("mp_admin profile 请求失败"))?;
    *cookies = merge_cookies(cookies, &home);
    let html = home.text().await.unwrap_or_default();
    let mut profile = extract_home_profile(&html);
    if profile.nickname.is_some()
        && let Ok(search) = search_biz(
            client,
            token,
            profile.nickname.as_deref().unwrap_or(""),
            cookies,
        )
        .await
    {
        if profile.fakeid.is_none() {
            profile.fakeid = search.fakeid;
        }
        if profile.nickname.is_none() {
            profile.nickname = search.nickname;
        }
    }
    Ok(profile)
}

fn extract_home_profile(html: &str) -> HomeProfile {
    let mut profile = HomeProfile::default();
    for pattern in [
        r#"nick_name\s*[:=]\s*['"]([^'"]+)['"]"#,
        r#"nickname\s*[:=]\s*['"]([^'"]+)['"]"#,
        r#"operator_nickname\s*[:=]\s*['"]([^'"]+)['"]"#,
    ] {
        if let Ok(re) = regex::Regex::new(pattern)
            && let Some(caps) = re.captures(html)
        {
            profile.nickname = caps.get(1).map(|m| m.as_str().trim().to_string());
            break;
        }
    }
    for pattern in [
        r#"fakeid\s*[:=]\s*['"]([^'"]+)['"]"#,
        r#"fakeId\s*[:=]\s*['"]([^'"]+)['"]"#,
    ] {
        if let Ok(re) = regex::Regex::new(pattern)
            && let Some(caps) = re.captures(html)
        {
            profile.fakeid = caps.get(1).map(|m| m.as_str().trim().to_string());
            break;
        }
    }
    profile
}

async fn search_biz(
    client: &Client,
    token: &str,
    query: &str,
    cookies: &mut HashMap<String, String>,
) -> ConnectorResult<HomeProfile> {
    let response = with_cookies(
        client
            .get(format!("{}/cgi-bin/searchbiz", base_url()))
            .query(&[
                ("action", "search_biz"),
                ("token", token),
                ("lang", "zh_CN"),
                ("f", "json"),
                ("ajax", "1"),
                ("random", "0.1"),
                ("query", query),
                ("begin", "0"),
                ("count", "5"),
            ]),
        cookies,
    )
    .send()
    .await
    .map_err(|_| ConnectorError::failed("mp_admin 搜索请求失败"))?;
    *cookies = merge_cookies(cookies, &response);
    let payload: Value = response.json().await.unwrap_or(Value::Null);
    Ok(parse_searchbiz(&payload))
}

fn parse_searchbiz(payload: &Value) -> HomeProfile {
    let entries = payload
        .get("list")
        .or_else(|| payload.get("data").and_then(|d| d.get("list")))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let first = entries.first().and_then(|v| v.as_object());
    HomeProfile {
        fakeid: first
            .and_then(|o| o.get("fakeid").or_else(|| o.get("fakeId")))
            .and_then(|v| v.as_str())
            .map(str::to_string),
        nickname: first
            .and_then(|o| {
                o.get("nickname")
                    .or_else(|| o.get("nick_name"))
                    .or_else(|| o.get("name"))
            })
            .and_then(|v| v.as_str())
            .map(str::to_string),
    }
}

pub async fn discover_recent_articles(
    creds: &MpAdminCredentials,
    source_fakeid: &str,
    limit: u8,
    start_cursor: u32,
    page_budget: u8,
) -> ConnectorResult<Vec<ArticleCandidate>> {
    if creds.token.is_empty() || creds.cookies.is_empty() {
        return Err(ConnectorError::needs_relogin(
            "mp_admin 凭据缺失，请重新扫码",
        ));
    }
    let client = http_client(creds.proxy_url.as_deref())?;
    let max_results = usize::from(limit).clamp(1, MP_ADMIN_PAGE_SIZE * MAX_INCREMENTAL_PAGES);
    let max_pages = usize::from(page_budget.max(1)).min(MAX_INCREMENTAL_PAGES);
    let mut cursor = start_cursor as usize;
    let mut pages_fetched = 0usize;
    let mut seen = HashSet::new();
    let mut candidates = Vec::with_capacity(max_results);

    while candidates.len() < max_results && pages_fetched < max_pages {
        let page_size = (max_results - candidates.len()).min(MP_ADMIN_PAGE_SIZE);
        let count = page_size.to_string();
        let begin = cursor.to_string();
        let response = with_cookies(
            client
                .get(format!("{}/cgi-bin/appmsgpublish", base_url()))
                .query(&[
                    ("sub", "list"),
                    ("search_field", "null"),
                    ("sub_action", "list_ex"),
                    ("fakeid", source_fakeid),
                    ("token", creds.token.as_str()),
                    ("lang", "zh_CN"),
                    ("f", "json"),
                    ("ajax", "1"),
                    ("count", count.as_str()),
                    ("begin", begin.as_str()),
                    ("query", ""),
                    ("type", "101_1"),
                    ("free_publish_type", "1"),
                ]),
            &creds.cookies,
        )
        .send()
        .await
        .map_err(|_| ConnectorError::failed("mp_admin 列表请求失败"))?;

        if !response.status().is_success() {
            return Err(ConnectorError::failed(format!(
                "mp_admin 列表 HTTP {}",
                response.status()
            )));
        }
        let payload: Value = response
            .json()
            .await
            .map_err(|_| ConnectorError::failed("mp_admin 列表响应格式无效"))?;
        if let Some(error) = base_resp_connector_error(&payload) {
            return Err(error);
        }

        let page = parse_appmsg_page(&payload, page_size, cursor as u32, Utc::now());
        let page_candidate_count = page.candidates.len();
        for candidate in page.candidates {
            if seen.insert(candidate_dedup_key(&candidate)) {
                candidates.push(candidate);
                if candidates.len() == max_results {
                    break;
                }
            }
        }
        pages_fetched += 1;
        cursor += page_size;
        if !page.can_continue || page_candidate_count == 0 {
            break;
        }
    }

    Ok(candidates)
}

#[cfg(test)]
fn parse_appmsg_list(payload: &Value, limit: usize) -> Vec<ArticleCandidate> {
    parse_appmsg_page(payload, limit, 0, Utc::now()).candidates
}

fn parse_appmsg_page(
    payload: &Value,
    limit: usize,
    cursor: u32,
    observed_at: chrono::DateTime<Utc>,
) -> ParsedAppmsgPage {
    let mut items = payload
        .get("app_msg_list")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    if items.is_empty()
        && let Some(page) = payload.get("publish_page").and_then(|v| v.as_str())
        && let Ok(parsed) = serde_json::from_str::<Value>(page)
    {
        for publish_item in parsed
            .get("publish_list")
            .or_else(|| parsed.get("list"))
            .and_then(|v| v.as_array())
            .into_iter()
            .flatten()
        {
            let mut ext = publish_item
                .get("appmsgex")
                .or_else(|| publish_item.get("app_msg_ext_info"))
                .cloned()
                .unwrap_or(Value::Null);
            if ext.is_string() {
                ext = serde_json::from_str(ext.as_str().unwrap_or("[]")).unwrap_or(Value::Null);
            }
            let ext_list = if ext.is_array() {
                ext.as_array().cloned().unwrap_or_default()
            } else if ext.is_object() {
                vec![ext]
            } else {
                vec![]
            };
            for item in ext_list {
                items.push(merge_publish_item(&item, publish_item));
            }
        }
    }

    let candidates = items
        .into_iter()
        .filter_map(|item| {
            let title = item
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or("无标题")
                .to_string();
            let link = item
                .get("link")
                .or_else(|| item.get("content_url"))
                .and_then(|v| v.as_str())
                .map(sanitize_article_url)
                .filter(|u| !u.is_empty())?;
            let published_at = item
                .get("update_time")
                .or_else(|| item.get("create_time"))
                .and_then(|v| {
                    v.as_i64()
                        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
                })
                .and_then(|ts| Utc.timestamp_opt(ts, 0).single());
            let author = string_field(&item, &["author", "author_name", "nickname"]);
            let cover_url = string_field(&item, &["cover", "cover_url", "cover_pic", "thumb_url"])
                .map(|url| sanitize_article_url(&url))
                .filter(|url| !url.is_empty());
            let identity = string_field(&item, &["appmsgid", "msgid"]);
            let evidence = ArticleCandidateEvidence {
                connector: NAME_MP_ADMIN.to_string(),
                cursor,
                response_summary_hash: candidate_summary_hash(
                    &title,
                    &link,
                    published_at,
                    identity.as_deref(),
                    author.as_deref(),
                    cover_url.as_deref(),
                ),
                observed_at,
            };
            Some(ArticleCandidate {
                title,
                canonical_url: link,
                published_at,
                identity,
                author,
                cover_url,
                evidence: Some(evidence),
            })
        })
        .take(limit)
        .collect();

    ParsedAppmsgPage {
        candidates,
        can_continue: can_msg_continue(payload),
    }
}

fn string_field(item: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        item.get(*key).and_then(|value| match value {
            Value::String(value) if !value.trim().is_empty() => Some(value.trim().to_string()),
            Value::Number(value) => Some(value.to_string()),
            _ => None,
        })
    })
}

fn can_msg_continue(payload: &Value) -> bool {
    payload
        .get("can_msg_continue")
        .or_else(|| payload.get("can_continue"))
        .and_then(|value| match value {
            Value::Bool(value) => Some(*value),
            Value::Number(value) => value.as_i64().map(|value| value != 0),
            Value::String(value) => Some(matches!(value.trim(), "1" | "true" | "TRUE")),
            _ => None,
        })
        .unwrap_or(false)
}

fn candidate_dedup_key(candidate: &ArticleCandidate) -> String {
    match candidate
        .identity
        .as_deref()
        .filter(|identity| !identity.trim().is_empty())
    {
        Some(identity) => format!("identity:{identity}"),
        None => format!("url:{}", candidate.canonical_url),
    }
}

fn candidate_summary_hash(
    title: &str,
    canonical_url: &str,
    published_at: Option<chrono::DateTime<Utc>>,
    identity: Option<&str>,
    author: Option<&str>,
    cover_url: Option<&str>,
) -> String {
    let summary = serde_json::json!({
        "title": title,
        "canonical_url": canonical_url,
        "published_at": published_at.map(|value| value.timestamp()),
        "identity": identity,
        "author": author,
        "cover_url": cover_url,
    });
    let serialized = serde_json::to_vec(&summary).expect("candidate summary JSON is serializable");
    format!("{:x}", Sha256::digest(serialized))
}

fn merge_publish_item(item: &Value, publish_item: &Value) -> Value {
    let mut obj = item.as_object().cloned().unwrap_or_default();
    if !obj.contains_key("update_time")
        && let Some(t) = publish_item.get("publish_time")
    {
        obj.insert("update_time".into(), t.clone());
    }
    if !obj.contains_key("link")
        && let Some(l) = item.get("content_url")
    {
        obj.insert("link".into(), l.clone());
    }
    Value::Object(obj)
}

/// 去掉 URL 上的 token 等登录参数。
pub fn sanitize_article_url(url: &str) -> String {
    let mut out = url.trim().to_string();
    if out.starts_with("//") {
        out = format!("https:{out}");
    }
    if let Ok(mut parsed) = url::Url::parse(&out) {
        let forbidden = ["token", "pass_ticket", "key", "uin"];
        if let Some(q) = parsed.query() {
            let kept: Vec<_> = q
                .split('&')
                .filter(|pair| {
                    forbidden
                        .iter()
                        .all(|f| !pair.starts_with(&format!("{f}=")))
                })
                .collect();
            if kept.is_empty() {
                parsed.set_query(None);
            } else {
                parsed.set_query(Some(&kept.join("&")));
            }
        }
        return parsed.to_string();
    }
    out.split('#').next().unwrap_or(&out).to_string()
}

pub fn parse_credentials(json: &str) -> ConnectorResult<MpAdminCredentials> {
    serde_json::from_str(json)
        .map_err(|e| ConnectorError::failed(format!("mp_admin 凭据 JSON 无效: {e}")))
}

/// 对后台首页执行最小登录态探测，不枚举文章或公众号。
pub async fn probe_credentials(creds: &MpAdminCredentials) -> ConnectorResult<()> {
    if creds.token.trim().is_empty() || creds.cookies.is_empty() {
        return Err(ConnectorError::needs_relogin(
            "mp_admin 凭据缺失，请重新扫码",
        ));
    }
    let client = http_client(creds.proxy_url.as_deref())?;
    let response = with_cookies(
        client
            .get(format!("{}/cgi-bin/home", base_url()))
            .query(&[("token", creds.token.as_str()), ("lang", "zh_CN")]),
        &creds.cookies,
    )
    .send()
    .await
    .map_err(|_| ConnectorError::failed("mp_admin 登录态探测请求失败"))?;
    if !response.status().is_success() {
        return Err(ConnectorError::failed("mp_admin 登录态探测响应异常"));
    }
    let final_url = response.url().path().to_ascii_lowercase();
    let body = response
        .text()
        .await
        .unwrap_or_default()
        .to_ascii_lowercase();
    if final_url.contains("login")
        || body.contains("loginpage")
        || body.contains("重新登录")
        || body.contains("请登录")
    {
        return Err(ConnectorError::needs_relogin("mp_admin 登录态已失效"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_strips_token_query() {
        let url = "https://mp.weixin.qq.com/s/abc?token=secret&__biz=biz";
        let clean = sanitize_article_url(url);
        assert!(!clean.contains("token="));
        assert!(clean.contains("__biz="));
    }

    #[test]
    fn parse_ask_status_codes() {
        assert_eq!(
            parse_ask_status(&serde_json::json!({"status": 4})),
            QrAskStatus::Scanned
        );
        assert_eq!(
            parse_ask_status(&serde_json::json!({"status": 1})),
            QrAskStatus::Confirmed
        );
    }

    #[test]
    fn parse_appmsg_basic() {
        let payload = serde_json::json!({
            "app_msg_list": [
                {
                    "title": "Hello",
                    "link": "https://mp.weixin.qq.com/s/x?token=bad",
                    "update_time": 1_700_000_000
                }
            ]
        });
        let list = parse_appmsg_list(&payload, 3);
        assert_eq!(list.len(), 1);
        assert!(!list[0].canonical_url.contains("token="));
        assert_eq!(list[0].title, "Hello");
        assert!(list[0].evidence.is_some());
    }

    #[test]
    fn parses_candidate_metadata_and_sanitized_evidence() {
        let observed_at = Utc.timestamp_opt(1_700_000_000, 0).single().unwrap();
        let payload = serde_json::json!({
            "app_msg_list": [{
                "title": "Hello",
                "link": "https://mp.weixin.qq.com/s/x?__biz=biz&token=bad",
                "update_time": 1_700_000_000,
                "appmsgid": 12345,
                "author_name": "Author",
                "cover_pic": "https://mmbiz.qpic.cn/cover?pass_ticket=secret&id=1"
            }]
        });

        let page = parse_appmsg_page(&payload, 10, 20, observed_at);
        assert_eq!(page.candidates.len(), 1);
        let candidate = &page.candidates[0];
        assert_eq!(candidate.identity.as_deref(), Some("12345"));
        assert_eq!(candidate.author.as_deref(), Some("Author"));
        assert_eq!(
            candidate.cover_url.as_deref(),
            Some("https://mmbiz.qpic.cn/cover?id=1")
        );
        let evidence = candidate.evidence.as_ref().unwrap();
        assert_eq!(evidence.connector, NAME_MP_ADMIN);
        assert_eq!(evidence.cursor, 20);
        assert_eq!(evidence.observed_at, observed_at);
        assert_eq!(evidence.response_summary_hash.len(), 64);
        assert!(!evidence.response_summary_hash.contains("secret"));
    }

    #[test]
    fn parses_page_continuation_flags() {
        let continue_page = parse_appmsg_page(
            &serde_json::json!({ "can_msg_continue": 1, "app_msg_list": [] }),
            10,
            0,
            Utc::now(),
        );
        assert!(continue_page.can_continue);

        let stop_page = parse_appmsg_page(
            &serde_json::json!({ "can_msg_continue": false, "app_msg_list": [] }),
            10,
            10,
            Utc::now(),
        );
        assert!(!stop_page.can_continue);
    }

    #[test]
    fn dedup_key_prefers_identity_and_falls_back_to_url() {
        let identity = ArticleCandidate {
            title: "One".into(),
            canonical_url: "https://mp.weixin.qq.com/s/one".into(),
            published_at: None,
            identity: Some("appmsg-1".into()),
            author: None,
            cover_url: None,
            evidence: None,
        };
        let same_identity_other_url = ArticleCandidate {
            canonical_url: "https://mp.weixin.qq.com/s/two".into(),
            ..identity.clone()
        };
        let no_identity = ArticleCandidate {
            identity: None,
            ..identity.clone()
        };

        assert_eq!(candidate_dedup_key(&identity), "identity:appmsg-1");
        assert_eq!(
            candidate_dedup_key(&same_identity_other_url),
            candidate_dedup_key(&identity)
        );
        assert_eq!(
            candidate_dedup_key(&no_identity),
            "url:https://mp.weixin.qq.com/s/one"
        );
    }

    #[test]
    fn classifies_platform_limits_and_expired_login() {
        let limited = base_resp_connector_error(&serde_json::json!({
            "base_resp": { "ret": 200003, "errmsg": "请求过于频繁" }
        }))
        .unwrap();
        assert_eq!(
            limited.reason_code,
            crate::error::codes::PLATFORM_RESTRICTED
        );
        assert!(!limited.message.contains("200003"));

        let expired = base_resp_connector_error(&serde_json::json!({
            "base_resp": { "ret": 200002, "errmsg": "invalid token" }
        }))
        .unwrap();
        assert_eq!(expired.reason_code, crate::error::codes::NEEDS_RELOGIN);
    }
}
