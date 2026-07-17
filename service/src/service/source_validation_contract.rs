//! Source 验证的字段校验、URL 身份解析与错误映射。

use server_core::AppError;

use crate::service::source_validation::PatchField;

pub(crate) fn validate_nonempty(field: &str, value: &str) -> Result<(), AppError> {
    if value.trim().is_empty() {
        return Err(AppError::new(
            "source.invalid_field",
            format!("{field} must not be empty"),
        ));
    }
    Ok(())
}

pub(crate) fn validate_patch_connector(
    field: &str,
    value: &PatchField<String>,
) -> Result<(), AppError> {
    if let PatchField::Value(value) = value {
        validate_nonempty(field, value)?;
    }
    Ok(())
}

pub(crate) fn patch_summary<T>(value: &PatchField<T>) -> &'static str {
    match value {
        PatchField::Missing => "unchanged",
        PatchField::Null => "cleared",
        PatchField::Value(_) => "configured",
    }
}

pub(crate) fn parse_wechat_from_url(
    url: &str,
) -> (
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
) {
    let Ok(parsed) = url::Url::parse(url) else {
        return (None, None, None, None);
    };
    let mut biz = None;
    let mut mid = None;
    let mut idx = None;
    let mut sn = None;
    for (k, v) in parsed.query_pairs() {
        match k.as_ref() {
            "__biz" => biz = Some(v.into_owned()),
            "mid" => mid = Some(v.into_owned()),
            "idx" => idx = Some(v.into_owned()),
            "sn" => sn = Some(v.into_owned()),
            _ => {}
        }
    }
    (biz, mid, idx, sn)
}

pub(crate) fn validate_hhmm(field: &str, value: &str) -> Result<(), AppError> {
    let parts: Vec<_> = value.split(':').collect();
    if parts.len() != 2 {
        return Err(AppError::new(
            "source.invalid_windows",
            format!("{field} 必须为 HH:MM"),
        ));
    }
    if parts[0].len() != 2 || parts[1].len() != 2 {
        return Err(AppError::new(
            "source.invalid_windows",
            format!("{field} must use two-digit HH:mm"),
        ));
    }
    let h: u32 = parts[0]
        .parse()
        .map_err(|_| AppError::new("source.invalid_windows", format!("{field} 小时无效")))?;
    let m: u32 = parts[1]
        .parse()
        .map_err(|_| AppError::new("source.invalid_windows", format!("{field} 分钟无效")))?;
    if h > 23 || m > 59 {
        return Err(AppError::new(
            "source.invalid_windows",
            format!("{field} 超出范围"),
        ));
    }
    Ok(())
}

pub(crate) fn map_repo(e: String) -> AppError {
    if let Some((code, msg)) = e.split_once(": ")
        && (code.starts_with("source.") || code.starts_with("connector."))
    {
        let status = if code.contains("not_found") {
            axum::http::StatusCode::NOT_FOUND
        } else if code.contains("conflict") {
            axum::http::StatusCode::CONFLICT
        } else {
            axum::http::StatusCode::BAD_REQUEST
        };
        return AppError::new(code, msg).with_status(status);
    }
    AppError::internal(e)
}
