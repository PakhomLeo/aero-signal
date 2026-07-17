//! QR 登录的状态解析、ticket 生成和公开响应映射。

use rand::RngCore;
use server_core::AppError;

use crate::adapters::connectors::{ConnectorError, QrPollStatus};
use crate::domain::collector::{
    CollectorAccount, CollectorAccountPublic, CollectorAccountStatus, CollectorEgressBinding,
};

pub(crate) fn parse_advance_status(raw: &str) -> Result<QrPollStatus, AppError> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "scanned" => Ok(QrPollStatus::Scanned),
        "confirmed" => Ok(QrPollStatus::Confirmed),
        other => Err(AppError::new(
            "collector.invalid_advance_status",
            format!("不支持的推进状态: {other}（仅 scanned|confirmed）"),
        )),
    }
}

pub(crate) fn generate_ticket() -> String {
    let mut bytes = [0u8; 24];
    rand::thread_rng().fill_bytes(&mut bytes);
    base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, bytes)
}

pub(crate) fn map_connector_err(e: ConnectorError) -> AppError {
    AppError::new(e.reason_code, e.message).with_status(axum::http::StatusCode::BAD_REQUEST)
}

pub(crate) fn to_public(
    account: &CollectorAccount,
    binding: Option<&CollectorEgressBinding>,
) -> CollectorAccountPublic {
    CollectorAccountPublic {
        id: account.id,
        connector: account.connector.clone(),
        display_name: account.display_name.clone(),
        external_account_ref: account.external_account_ref.clone(),
        status: account.status,
        fixed_egress_id: binding.and_then(|b| b.fixed_egress_id),
        requires_relogin: binding.map(|b| b.requires_relogin).unwrap_or(false)
            || account.status == CollectorAccountStatus::NeedsRelogin,
        last_probe_at: account.last_probe_at,
        last_success_at: account.last_success_at,
        last_failure_at: account.last_failure_at,
        last_error_code: account.last_error_code.clone(),
        created_at: account.created_at,
        updated_at: account.updated_at,
    }
}
