//! QrLoginService：二维码 ticket 生命周期、CAS finalize、probe 落库。

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{Value, json};
use server_core::{AppError, Clock};
use uuid::Uuid;

use crate::adapters::connectors::{
    CollectorAccountView, ConnectorBackend, ConnectorCapability, ConnectorRegistry, LoginContext,
    ProbeStatus, QrPollStatus,
};
use crate::adapters::qr_ticket_store::{QrTicketRecord, QrTicketStore};
use crate::adapters::secret_store::SecretStore;
use crate::audit::{ACTOR_SYSTEM_ADMINISTRATOR, AuditWriter};
use crate::domain::collector::{
    CollectorAccount, CollectorAccountPublic, CollectorAccountStatus, CollectorEgressBinding,
    CollectorLoginSession, QrLoginStatus, validate_connector_name,
};
use crate::domain::egress::{EgressKind, EgressPurpose, EgressStatus};
use crate::repository::collector::CollectorRepository;
use crate::service::AuthSession;
use crate::service::egress::EgressService;
use crate::service::qr_login_contract::{
    generate_ticket, map_connector_err, parse_advance_status, to_public,
};

/// 创建二维码会话请求。
#[derive(Debug, Deserialize)]
pub struct CreateQrSessionCommand {
    pub connector: String,
    pub fixed_egress_id: Uuid,
}

/// 二维码会话 API 响应（无 cookie/token）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct QrSessionResponse {
    pub ticket: String,
    pub qr_data_url: String,
    pub expires_at: DateTime<Utc>,
    pub poll_after_ms: u64,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account: Option<CollectorAccountPublic>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// PATCH 采集账号。
#[derive(Debug, Deserialize)]
pub struct PatchCollectorAccountCommand {
    pub display_name: Option<String>,
    pub status: Option<String>,
    /// 变更固定出口 → needs_relogin（禁止静默换绑）。
    pub fixed_egress_id: Option<Uuid>,
}

/// 重新授权：标记 needs_relogin，客户端再走 create QR。
#[derive(Debug, Deserialize, Default)]
pub struct ReauthorizeCommand {
    pub fixed_egress_id: Option<Uuid>,
}

/// Fake/dev：推进二维码会话状态。
#[derive(Debug, Deserialize)]
pub struct AdvanceQrSessionCommand {
    /// `scanned` | `confirmed`（映射到 QrPollStatus）
    pub status: String,
}

/// 采集登录服务。
pub struct QrLoginService {
    tickets: Arc<dyn QrTicketStore>,
    collectors: Arc<dyn CollectorRepository>,
    secrets: Arc<dyn SecretStore>,
    connectors: Arc<ConnectorRegistry>,
    egress: Arc<EgressService>,
    clock: Arc<dyn Clock>,
    audit: Option<AuditWriter>,
}

impl QrLoginService {
    pub fn new(
        tickets: Arc<dyn QrTicketStore>,
        collectors: Arc<dyn CollectorRepository>,
        secrets: Arc<dyn SecretStore>,
        connectors: Arc<ConnectorRegistry>,
        egress: Arc<EgressService>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            tickets,
            collectors,
            secrets,
            connectors,
            egress,
            clock,
            audit: None,
        }
    }

    pub fn with_audit(mut self, audit: AuditWriter) -> Self {
        self.audit = Some(audit);
        self
    }

    fn now_utc(&self) -> DateTime<Utc> {
        DateTime::<Utc>::from(self.clock.now())
    }

    fn backend(&self, name: &str) -> Result<Arc<dyn ConnectorBackend>, AppError> {
        self.connectors
            .get(name)
            .ok_or_else(|| AppError::new("connector.unsupported", format!("未注册连接器: {name}")))
    }

    async fn validate_fixed_egress_for_login(&self, id: Uuid) -> Result<(), AppError> {
        let profile = self
            .egress
            .get_egress_profile(id)
            .await?
            .ok_or_else(|| AppError::not_found("egress.not_found", "固定出口不存在"))?;
        if profile.status != EgressStatus::Active {
            return Err(AppError::new("egress.inactive", "出口未启用，无法用于登录"));
        }
        match profile.kind {
            EgressKind::Fixed | EgressKind::LocalDirect => {}
            EgressKind::Dynamic => {
                return Err(AppError::new(
                    "egress.purpose_mismatch",
                    "动态出口不能用于登录",
                ));
            }
        }
        if !profile.allowed_purposes.contains(&EgressPurpose::Login) {
            return Err(AppError::new(
                "egress.purpose_mismatch",
                "出口未允许 login 用途",
            ));
        }
        Ok(())
    }

    /// 创建二维码会话。
    pub async fn create_qr_session(
        &self,
        auth: &AuthSession,
        cmd: CreateQrSessionCommand,
    ) -> Result<QrSessionResponse, AppError> {
        validate_connector_name(&cmd.connector)
            .map_err(|m| AppError::new("connector.unsupported", m))?;
        self.validate_fixed_egress_for_login(cmd.fixed_egress_id)
            .await?;

        let backend = self.backend(&cmd.connector)?;
        if !backend
            .capabilities()
            .contains(ConnectorCapability::QrLogin)
        {
            return Err(AppError::new(
                "connector.unsupported",
                format!("连接器 {} 不支持扫码登录", cmd.connector),
            ));
        }

        let ticket = generate_ticket();
        let proxy_url = self.egress.resolve_login_proxy(cmd.fixed_egress_id).await?;
        let ctx = LoginContext {
            ticket: ticket.clone(),
            fixed_egress_id: Some(cmd.fixed_egress_id),
            administrator_session_hash: auth.session_hash.clone(),
            proxy_url,
        };
        let qr = backend
            .create_qr_session(ctx)
            .await
            .map_err(map_connector_err)?;

        let now = self.now_utc();
        let expires_at = qr.expires_at;
        if expires_at <= now {
            return Err(AppError::new("collector.qr_expired", "二维码已过期"));
        }
        let ttl_secs = (expires_at - now).num_seconds().max(1) as u64;

        // connector 私有状态只进 SecretStore；Redis/内存 ticket 仅保存引用。
        let provider_state_secret_ref =
            match qr.provider_state_json.as_deref() {
                Some(state) => Some(self.secrets.put(state).await.map_err(|_| {
                    AppError::new("secret_store.write_failed", "保存二维码状态失败")
                })?),
                None => None,
            };

        let record = QrTicketRecord {
            ticket: ticket.clone(),
            administrator_session_hash: auth.session_hash.clone(),
            connector: cmd.connector,
            fixed_egress_id: cmd.fixed_egress_id,
            provider_state_secret_ref,
            qr_data_url: qr.qr_data_url.clone(),
            poll_after_ms: qr.poll_after_ms,
            status: QrLoginStatus::QrWaiting,
            expires_at,
            created_at: now,
            collector_account_id: None,
            display_name: None,
            external_account_ref: None,
            last_error_code: None,
        };
        if let Err(error) = self
            .tickets
            .put(&record, Duration::from_secs(ttl_secs))
            .await
        {
            self.cleanup_ticket_secrets(&record).await;
            return Err(AppError::new("collector.qr_ticket_store_failed", error));
        }

        if let Some(ref audit) = self.audit {
            let _ = audit
                .success(
                    ACTOR_SYSTEM_ADMINISTRATOR,
                    Some(&auth.username),
                    "collector.qr_session_created",
                    Some("qr_session"),
                    Some(&ticket),
                    json!({
                        "connector": record.connector,
                        "fixed_egress_id": cmd.fixed_egress_id.to_string(),
                        "status": QrLoginStatus::QrWaiting.as_str(),
                    }),
                    now,
                )
                .await;
        }

        Ok(QrSessionResponse {
            ticket,
            qr_data_url: qr.qr_data_url,
            expires_at,
            poll_after_ms: qr.poll_after_ms,
            status: QrLoginStatus::QrWaiting.as_str().into(),
            account: None,
            message: None,
        })
    }

    fn ensure_ticket_owner(
        &self,
        record: &QrTicketRecord,
        auth: &AuthSession,
    ) -> Result<(), AppError> {
        if record.administrator_session_hash != auth.session_hash {
            return Err(AppError::forbidden("无权访问该二维码会话"));
        }
        Ok(())
    }

    async fn load_owned_ticket(
        &self,
        auth: &AuthSession,
        ticket: &str,
    ) -> Result<QrTicketRecord, AppError> {
        let record = self
            .tickets
            .get(ticket)
            .await
            .map_err(AppError::internal)?
            .ok_or_else(|| {
                AppError::not_found("collector.qr_not_found", "二维码会话不存在或已过期")
            })?;
        self.ensure_ticket_owner(&record, auth)?;
        Ok(record)
    }

    async fn cleanup_ticket_secrets(&self, record: &QrTicketRecord) {
        if let Some(r) = &record.provider_state_secret_ref {
            let _ = self.secrets.delete(r).await;
        }
    }

    async fn load_provider_state(
        &self,
        record: &QrTicketRecord,
    ) -> Result<Option<String>, AppError> {
        let Some(secret_ref) = record.provider_state_secret_ref.as_deref() else {
            return Ok(None);
        };
        self.secrets
            .get(secret_ref)
            .await
            .map_err(|_| AppError::new("secret_store.read_failed", "读取二维码状态失败"))?
            .map(Some)
            .ok_or_else(|| {
                AppError::new(
                    "collector.provider_state_missing",
                    "二维码状态已丢失，请重新扫码",
                )
            })
    }

    async fn replace_provider_state(
        &self,
        record: &mut QrTicketRecord,
        next_state: Option<String>,
    ) -> Result<(), AppError> {
        let Some(next_state) = next_state else {
            return Ok(());
        };
        let next_ref = self
            .secrets
            .put(&next_state)
            .await
            .map_err(|_| AppError::new("secret_store.write_failed", "更新二维码状态失败"))?;
        let previous_ref = record.provider_state_secret_ref.replace(next_ref.clone());
        if let Err(error) = self.tickets.update(record).await {
            let _ = self.secrets.delete(&next_ref).await;
            record.provider_state_secret_ref = previous_ref;
            return Err(AppError::new("collector.qr_state_update_failed", error));
        }
        if let Some(previous_ref) = previous_ref {
            let _ = self.secrets.delete(&previous_ref).await;
        }
        Ok(())
    }

    async fn mark_failed_and_cleanup(
        &self,
        mut record: QrTicketRecord,
        code: &str,
    ) -> Result<QrSessionResponse, AppError> {
        let _ = record.status.transition(QrLoginStatus::Failed);
        record.status = QrLoginStatus::Failed;
        record.last_error_code = Some(code.into());
        self.cleanup_ticket_secrets(&record).await;
        let _ = self.tickets.update(&record).await;
        let _ = self.tickets.delete(&record.ticket).await;
        Ok(QrSessionResponse {
            ticket: record.ticket,
            qr_data_url: record.qr_data_url,
            expires_at: record.expires_at,
            poll_after_ms: record.poll_after_ms,
            status: QrLoginStatus::Failed.as_str().into(),
            account: None,
            message: Some(code.into()),
        })
    }

    async fn account_public(&self, id: Uuid) -> Result<Option<CollectorAccountPublic>, AppError> {
        let Some(account) = self
            .collectors
            .get_account(id)
            .await
            .map_err(AppError::internal)?
        else {
            return Ok(None);
        };
        let binding = self
            .collectors
            .get_egress_binding(id)
            .await
            .map_err(AppError::internal)?;
        Ok(Some(to_public(&account, binding.as_ref())))
    }

    /// 轮询；confirmed 时 CAS → finalizing → finalize + probe → 落库。
    pub async fn poll_qr_session(
        &self,
        auth: &AuthSession,
        ticket: &str,
    ) -> Result<QrSessionResponse, AppError> {
        let mut record = self.load_owned_ticket(auth, ticket).await?;
        let now = self.now_utc();

        // 已 active：幂等返回账号摘要
        if record.status == QrLoginStatus::Active {
            let account = if let Some(id) = record.collector_account_id {
                self.account_public(id).await?
            } else {
                None
            };
            return Ok(QrSessionResponse {
                ticket: record.ticket,
                qr_data_url: record.qr_data_url,
                expires_at: record.expires_at,
                poll_after_ms: record.poll_after_ms,
                status: QrLoginStatus::Active.as_str().into(),
                account,
                message: None,
            });
        }

        if record.status.is_terminal() {
            return Ok(QrSessionResponse {
                ticket: record.ticket,
                qr_data_url: record.qr_data_url,
                expires_at: record.expires_at,
                poll_after_ms: record.poll_after_ms,
                status: record.status.as_str().into(),
                account: None,
                message: record.last_error_code,
            });
        }

        if now >= record.expires_at
            && matches!(
                record.status,
                QrLoginStatus::QrWaiting | QrLoginStatus::QrScanned
            )
        {
            record.status = record
                .status
                .transition(QrLoginStatus::Expired)
                .map_err(|m| AppError::new("collector.invalid_transition", m))?;
            self.cleanup_ticket_secrets(&record).await;
            let _ = self.tickets.update(&record).await;
            return Err(AppError::new("collector.qr_expired", "二维码已过期"));
        }

        let backend = self.backend(&record.connector)?;
        let provider_state = match self.load_provider_state(&record).await {
            Ok(state) => state,
            Err(error) => {
                return self.mark_failed_and_cleanup(record, &error.code).await;
            }
        };
        let poll = backend
            .poll_qr_session(ticket, provider_state.as_deref())
            .await
            .map_err(map_connector_err)?;
        self.replace_provider_state(&mut record, poll.provider_state_json.clone())
            .await?;

        match poll.status {
            QrPollStatus::Waiting => Ok(QrSessionResponse {
                ticket: record.ticket,
                qr_data_url: record.qr_data_url,
                expires_at: record.expires_at,
                poll_after_ms: record.poll_after_ms,
                status: QrLoginStatus::QrWaiting.as_str().into(),
                account: None,
                message: poll.message,
            }),
            QrPollStatus::Scanned => {
                if record.status == QrLoginStatus::QrWaiting {
                    let _ = self
                        .tickets
                        .compare_and_set_status(
                            ticket,
                            QrLoginStatus::QrWaiting,
                            QrLoginStatus::QrScanned,
                        )
                        .await
                        .map_err(AppError::internal)?;
                    record.status = QrLoginStatus::QrScanned;
                }
                Ok(QrSessionResponse {
                    ticket: record.ticket,
                    qr_data_url: record.qr_data_url,
                    expires_at: record.expires_at,
                    poll_after_ms: record.poll_after_ms,
                    status: QrLoginStatus::QrScanned.as_str().into(),
                    account: None,
                    message: poll.message,
                })
            }
            QrPollStatus::Expired => {
                let _ = record.status.transition(QrLoginStatus::Expired);
                record.status = QrLoginStatus::Expired;
                self.cleanup_ticket_secrets(&record).await;
                let _ = self.tickets.update(&record).await;
                Err(AppError::new("collector.qr_expired", "二维码已过期"))
            }
            QrPollStatus::Cancelled => {
                record.status = QrLoginStatus::Cancelled;
                self.cleanup_ticket_secrets(&record).await;
                let _ = self.tickets.update(&record).await;
                Ok(QrSessionResponse {
                    ticket: record.ticket,
                    qr_data_url: record.qr_data_url,
                    expires_at: record.expires_at,
                    poll_after_ms: record.poll_after_ms,
                    status: "cancelled".into(),
                    account: None,
                    message: poll.message,
                })
            }
            QrPollStatus::Failed => {
                self.mark_failed_and_cleanup(record, "collector.qr_failed")
                    .await
            }
            QrPollStatus::Confirmed => self.finalize_confirmed(auth, record).await,
        }
    }

    async fn finalize_confirmed(
        &self,
        auth: &AuthSession,
        record: QrTicketRecord,
    ) -> Result<QrSessionResponse, AppError> {
        let ticket = record.ticket.clone();

        // CAS：qr_waiting/qr_scanned → finalizing；失败则返回并发/幂等结果
        let cas = self.cas_enter_finalizing(&ticket, record.status).await?;
        let Some(mut record) = cas else {
            return self.poll_after_lost_cas(&ticket).await;
        };

        if let Err(e) = self
            .validate_fixed_egress_for_login(record.fixed_egress_id)
            .await
        {
            self.cleanup_ticket_secrets(&record).await;
            let _ = self.tickets.delete(&ticket).await;
            return Err(e);
        }

        let backend = self.backend(&record.connector)?;
        let provider_state = match self.load_provider_state(&record).await {
            Ok(state) => state,
            Err(error) => {
                return self.mark_failed_and_cleanup(record, &error.code).await;
            }
        };
        let cred = match backend
            .finalize_login(&ticket, provider_state.as_deref())
            .await
        {
            Ok(c) => c,
            Err(e) => return self.mark_failed_and_cleanup(record, &e.reason_code).await,
        };

        let credential_secret_ref_result = if let Some(payload) = cred.store_payload.as_deref() {
            self.secrets.put(payload).await
        } else {
            self.secrets.put(&format!("cred:{}", cred.secret_ref)).await
        };
        let credential_secret_ref = match credential_secret_ref_result {
            Ok(secret_ref) => secret_ref,
            Err(_) => {
                return self
                    .mark_failed_and_cleanup(record, "secret_store.write_failed")
                    .await;
            }
        };

        let now = self.now_utc();
        let account_id = Uuid::new_v4();
        let display_name = cred
            .display_name
            .clone()
            .unwrap_or_else(|| format!("{} account", record.connector));
        let external_ref = cred.account_identity.clone();

        let view = CollectorAccountView {
            id: account_id,
            connector: record.connector.clone(),
            display_name: Some(display_name.clone()),
            active: true,
            needs_relogin: false,
            fixed_egress_healthy: true,
            fixed_egress_id: Some(record.fixed_egress_id),
            cooling: false,
            budget_exhausted: false,
            enabled_capabilities: backend.capabilities(),
            authorized_mp_ids: cred.authorized_mp_ids.clone(),
        };

        let probe = match backend
            .probe(&view, cred.store_payload.as_deref(), now)
            .await
        {
            Ok(p) => p,
            Err(e) => {
                let _ = self.secrets.delete(&credential_secret_ref).await;
                return self.mark_failed_and_cleanup(record, &e.reason_code).await;
            }
        };
        if !matches!(probe.status, ProbeStatus::Healthy | ProbeStatus::Degraded) {
            let _ = self.secrets.delete(&credential_secret_ref).await;
            let code = probe
                .reason_code
                .unwrap_or_else(|| "collector.probe_failed".into());
            return self.mark_failed_and_cleanup(record, &code).await;
        }

        let caps_json: Value = json!({
            "items": backend.capabilities().items.iter().map(|c| c.as_str()).collect::<Vec<_>>(),
        });

        let account = CollectorAccount {
            id: account_id,
            connector: record.connector.clone(),
            display_name: display_name.clone(),
            external_account_ref: external_ref.clone(),
            status: CollectorAccountStatus::Active,
            capability_json: caps_json,
            last_probe_at: Some(probe.probed_at),
            last_success_at: Some(now),
            last_failure_at: None,
            last_error_code: None,
            cooldown_until: None,
            daily_budget_remaining: None,
            created_at: now,
            updated_at: now,
        };

        let session = CollectorLoginSession {
            id: Uuid::new_v4(),
            collector_account_id: account_id,
            credential_secret_ref: credential_secret_ref.clone(),
            browser_profile_ref: None,
            authorized_mp_ids: cred.authorized_mp_ids.clone(),
            expires_at: cred.expires_at,
            confirmed_at: now,
            revoked_at: None,
        };

        let binding = CollectorEgressBinding {
            collector_account_id: account_id,
            fixed_egress_id: Some(record.fixed_egress_id),
            bound_at: now,
            changed_at: None,
            requires_relogin: false,
        };

        if let Err(e) = self
            .collectors
            .persist_login_success(&account, &session, &binding, false)
            .await
        {
            let _ = self.secrets.delete(&credential_secret_ref).await;
            self.cleanup_ticket_secrets(&record).await;
            let _ = self.tickets.delete(&ticket).await;
            return Err(AppError::internal(e));
        }

        self.cleanup_ticket_secrets(&record).await;
        record.provider_state_secret_ref = None;
        record.status = QrLoginStatus::Active;
        record.collector_account_id = Some(account_id);
        record.display_name = Some(display_name);
        record.external_account_ref = external_ref;
        self.tickets
            .update(&record)
            .await
            .map_err(AppError::internal)?;

        if let Some(ref audit) = self.audit {
            let _ = audit
                .success(
                    ACTOR_SYSTEM_ADMINISTRATOR,
                    Some(&auth.username),
                    "collector.qr_session_confirmed",
                    Some("collector_account"),
                    Some(&account_id.to_string()),
                    json!({
                        "ticket": ticket,
                        "connector": account.connector,
                        "fixed_egress_id": record.fixed_egress_id.to_string(),
                        "status": "active",
                    }),
                    now,
                )
                .await;
        }

        Ok(QrSessionResponse {
            ticket: record.ticket,
            qr_data_url: record.qr_data_url,
            expires_at: record.expires_at,
            poll_after_ms: record.poll_after_ms,
            status: "active".into(),
            account: Some(to_public(&account, Some(&binding))),
            message: None,
        })
    }

    /// CAS 进入 finalizing；返回 Some(record) 表示本调用赢得 finalize。
    async fn cas_enter_finalizing(
        &self,
        ticket: &str,
        current: QrLoginStatus,
    ) -> Result<Option<QrTicketRecord>, AppError> {
        match current {
            QrLoginStatus::QrScanned => self
                .tickets
                .compare_and_set_status(ticket, QrLoginStatus::QrScanned, QrLoginStatus::Finalizing)
                .await
                .map_err(AppError::internal),
            QrLoginStatus::QrWaiting => {
                let _ = self
                    .tickets
                    .compare_and_set_status(
                        ticket,
                        QrLoginStatus::QrWaiting,
                        QrLoginStatus::QrScanned,
                    )
                    .await
                    .map_err(AppError::internal)?;
                self.tickets
                    .compare_and_set_status(
                        ticket,
                        QrLoginStatus::QrScanned,
                        QrLoginStatus::Finalizing,
                    )
                    .await
                    .map_err(AppError::internal)
            }
            QrLoginStatus::Finalizing | QrLoginStatus::Active => Ok(None),
            other => Err(AppError::new(
                "collector.invalid_transition",
                format!("无法从 {} 进入 finalizing", other.as_str()),
            )),
        }
    }

    async fn poll_after_lost_cas(&self, ticket: &str) -> Result<QrSessionResponse, AppError> {
        let latest = self
            .tickets
            .get(ticket)
            .await
            .map_err(AppError::internal)?
            .ok_or_else(|| AppError::not_found("collector.qr_not_found", "二维码会话不存在"))?;
        if latest.status == QrLoginStatus::Active {
            let account = if let Some(id) = latest.collector_account_id {
                self.account_public(id).await?
            } else {
                None
            };
            return Ok(QrSessionResponse {
                ticket: latest.ticket,
                qr_data_url: latest.qr_data_url,
                expires_at: latest.expires_at,
                poll_after_ms: latest.poll_after_ms,
                status: "active".into(),
                account,
                message: None,
            });
        }
        Ok(QrSessionResponse {
            ticket: latest.ticket,
            qr_data_url: latest.qr_data_url,
            expires_at: latest.expires_at,
            poll_after_ms: latest.poll_after_ms,
            status: latest.status.as_str().into(),
            account: None,
            message: Some("并发 finalize 中".into()),
        })
    }

    /// Fake/dev：推进二维码会话，再返回当前 poll 结果。
    pub async fn advance_qr_session(
        &self,
        auth: &AuthSession,
        ticket: &str,
        cmd: AdvanceQrSessionCommand,
    ) -> Result<QrSessionResponse, AppError> {
        let record = self.load_owned_ticket(auth, ticket).await?;
        if record.connector != crate::adapters::connectors::NAME_FAKE {
            return Err(AppError::new(
                "collector.advance_not_allowed",
                format!(
                    "仅 fake 连接器支持推进二维码状态，当前为 {}",
                    record.connector
                ),
            ));
        }
        let target = parse_advance_status(&cmd.status)?;
        let backend = self.backend(&record.connector)?;
        backend
            .advance_qr(ticket, target)
            .map_err(map_connector_err)?;
        self.poll_qr_session(auth, ticket).await
    }

    /// 取消会话并清理临时 secret。
    pub async fn cancel_qr_session(
        &self,
        auth: &AuthSession,
        ticket: &str,
    ) -> Result<QrSessionResponse, AppError> {
        let mut record = self.load_owned_ticket(auth, ticket).await?;
        if record.status == QrLoginStatus::Active {
            let account = if let Some(id) = record.collector_account_id {
                self.account_public(id).await?
            } else {
                None
            };
            return Ok(QrSessionResponse {
                ticket: record.ticket,
                qr_data_url: record.qr_data_url,
                expires_at: record.expires_at,
                poll_after_ms: record.poll_after_ms,
                status: "active".into(),
                account,
                message: Some("已完成，无法取消".into()),
            });
        }
        if !record.status.is_terminal() {
            record.status = record
                .status
                .transition(QrLoginStatus::Cancelled)
                .unwrap_or(QrLoginStatus::Cancelled);
        }
        self.cleanup_ticket_secrets(&record).await;
        let _ = self.tickets.update(&record).await;
        let _ = self.tickets.delete(&record.ticket).await;
        if let Some(ref audit) = self.audit {
            let _ = audit
                .success(
                    ACTOR_SYSTEM_ADMINISTRATOR,
                    Some(&auth.username),
                    "collector.qr_session_cancelled",
                    Some("qr_session"),
                    Some(&record.ticket),
                    json!({
                        "connector": record.connector,
                        "status": "cancelled",
                    }),
                    self.now_utc(),
                )
                .await;
        }
        Ok(QrSessionResponse {
            ticket: record.ticket.clone(),
            qr_data_url: record.qr_data_url,
            expires_at: record.expires_at,
            poll_after_ms: record.poll_after_ms,
            status: "cancelled".into(),
            account: None,
            message: None,
        })
    }

    pub async fn list_accounts(&self) -> Result<Vec<CollectorAccountPublic>, AppError> {
        let accounts = self
            .collectors
            .list_accounts()
            .await
            .map_err(AppError::internal)?;
        let mut out = Vec::with_capacity(accounts.len());
        for a in accounts {
            let b = self
                .collectors
                .get_egress_binding(a.id)
                .await
                .map_err(AppError::internal)?;
            out.push(to_public(&a, b.as_ref()));
        }
        Ok(out)
    }

    pub async fn patch_account(
        &self,
        id: Uuid,
        cmd: PatchCollectorAccountCommand,
    ) -> Result<CollectorAccountPublic, AppError> {
        let mut account = self
            .collectors
            .get_account(id)
            .await
            .map_err(AppError::internal)?
            .ok_or_else(|| AppError::not_found("collector.not_found", "采集账号不存在"))?;
        let now = self.now_utc();

        if let Some(name) = cmd.display_name {
            if name.trim().is_empty() {
                return Err(AppError::new("collector.invalid_name", "显示名不能为空"));
            }
            account.display_name = name.trim().to_string();
            account.updated_at = now;
        }

        if let Some(status_s) = cmd.status {
            let next = CollectorAccountStatus::parse(&status_s)
                .map_err(|m| AppError::new("collector.invalid_status", m))?;
            account
                .transition_status(next, now)
                .map_err(|m| AppError::new("collector.invalid_transition", m))?;
        }

        let mut binding = self
            .collectors
            .get_egress_binding(id)
            .await
            .map_err(AppError::internal)?
            .unwrap_or(CollectorEgressBinding {
                collector_account_id: id,
                fixed_egress_id: None,
                bound_at: now,
                changed_at: None,
                requires_relogin: false,
            });

        let mut egress_binding_changed = false;
        if let Some(new_egress) = cmd.fixed_egress_id {
            self.validate_fixed_egress_for_login(new_egress).await?;
            if binding.fixed_egress_id != Some(new_egress) {
                // 固定出口变更 → needs_relogin，禁止静默换绑继续用
                binding.fixed_egress_id = Some(new_egress);
                binding.changed_at = Some(now);
                binding.requires_relogin = true;
                egress_binding_changed = true;
                if account.status == CollectorAccountStatus::Active {
                    account
                        .transition_status(CollectorAccountStatus::NeedsRelogin, now)
                        .map_err(|m| AppError::new("collector.invalid_transition", m))?;
                }
                // 吊销旧登录会话
                self.collectors
                    .revoke_login_sessions(id, now)
                    .await
                    .map_err(AppError::internal)?;
            }
        }

        self.collectors
            .update_account(&account)
            .await
            .map_err(AppError::internal)?;
        self.collectors
            .upsert_egress_binding(&binding)
            .await
            .map_err(AppError::internal)?;

        if egress_binding_changed && let Some(ref audit) = self.audit {
            let _ = audit
                .success(
                    ACTOR_SYSTEM_ADMINISTRATOR,
                    None,
                    "collector.egress_binding_changed",
                    Some("collector_account"),
                    Some(&id.to_string()),
                    json!({
                        "fixed_egress_id": binding.fixed_egress_id.map(|e| e.to_string()),
                        "requires_relogin": binding.requires_relogin,
                    }),
                    now,
                )
                .await;
        }

        Ok(to_public(&account, Some(&binding)))
    }

    pub async fn probe_now(&self, id: Uuid) -> Result<CollectorAccountPublic, AppError> {
        let mut account = self
            .collectors
            .get_account(id)
            .await
            .map_err(AppError::internal)?
            .ok_or_else(|| AppError::not_found("collector.not_found", "采集账号不存在"))?;
        let binding = self
            .collectors
            .get_egress_binding(id)
            .await
            .map_err(AppError::internal)?;
        let now = self.now_utc();
        let backend = self.backend(&account.connector)?;
        let mut view = CollectorAccountView {
            id: account.id,
            connector: account.connector.clone(),
            display_name: Some(account.display_name.clone()),
            active: account.status == CollectorAccountStatus::Active,
            needs_relogin: account.status == CollectorAccountStatus::NeedsRelogin
                || binding
                    .as_ref()
                    .map(|b| b.requires_relogin)
                    .unwrap_or(false),
            fixed_egress_healthy: true,
            fixed_egress_id: binding.as_ref().and_then(|b| b.fixed_egress_id),
            cooling: false,
            budget_exhausted: false,
            enabled_capabilities: backend.capabilities(),
            authorized_mp_ids: Vec::new(),
        };
        let session = self
            .collectors
            .get_active_login_session(account.id)
            .await
            .map_err(AppError::internal)?
            .ok_or_else(|| AppError::new("collector.needs_relogin", "采集账号没有有效登录会话"))?;
        view.authorized_mp_ids = session.authorized_mp_ids.clone();
        let credentials = self
            .secrets
            .get(&session.credential_secret_ref)
            .await
            .map_err(|_| AppError::new("secret_store.read_failed", "读取采集账号凭据失败"))?
            .ok_or_else(|| AppError::new("collector.credential_missing", "采集账号凭据不存在"))?;
        let probe = backend
            .probe(&view, Some(&credentials), now)
            .await
            .map_err(map_connector_err)?;
        account.last_probe_at = Some(probe.probed_at);
        account.updated_at = now;
        match probe.status {
            ProbeStatus::Healthy | ProbeStatus::Degraded => {
                account.last_success_at = Some(now);
                account.last_error_code = None;
            }
            ProbeStatus::NeedsRelogin => {
                account.last_failure_at = Some(now);
                account.last_error_code = Some("collector.needs_relogin".into());
                if account.status == CollectorAccountStatus::Active {
                    let _ = account.transition_status(CollectorAccountStatus::NeedsRelogin, now);
                }
            }
            ProbeStatus::Failed | ProbeStatus::Unsupported => {
                account.last_failure_at = Some(now);
                account.last_error_code = probe.reason_code.clone();
            }
        }
        self.collectors
            .update_account(&account)
            .await
            .map_err(AppError::internal)?;
        Ok(to_public(&account, binding.as_ref()))
    }

    pub async fn reauthorize(
        &self,
        id: Uuid,
        cmd: ReauthorizeCommand,
    ) -> Result<CollectorAccountPublic, AppError> {
        let mut account = self
            .collectors
            .get_account(id)
            .await
            .map_err(AppError::internal)?
            .ok_or_else(|| AppError::not_found("collector.not_found", "采集账号不存在"))?;
        let now = self.now_utc();
        if account.status == CollectorAccountStatus::Active {
            account
                .transition_status(CollectorAccountStatus::NeedsRelogin, now)
                .map_err(|m| AppError::new("collector.invalid_transition", m))?;
        } else if account.status == CollectorAccountStatus::Disabled {
            account
                .transition_status(CollectorAccountStatus::NeedsRelogin, now)
                .map_err(|m| AppError::new("collector.invalid_transition", m))?;
        }

        let mut binding = self
            .collectors
            .get_egress_binding(id)
            .await
            .map_err(AppError::internal)?
            .unwrap_or(CollectorEgressBinding {
                collector_account_id: id,
                fixed_egress_id: None,
                bound_at: now,
                changed_at: None,
                requires_relogin: true,
            });
        binding.requires_relogin = true;
        let mut egress_binding_changed = false;
        if let Some(eid) = cmd.fixed_egress_id {
            self.validate_fixed_egress_for_login(eid).await?;
            if binding.fixed_egress_id != Some(eid) {
                binding.fixed_egress_id = Some(eid);
                binding.changed_at = Some(now);
                egress_binding_changed = true;
            }
        }
        self.collectors
            .revoke_login_sessions(id, now)
            .await
            .map_err(AppError::internal)?;
        self.collectors
            .update_account(&account)
            .await
            .map_err(AppError::internal)?;
        self.collectors
            .upsert_egress_binding(&binding)
            .await
            .map_err(AppError::internal)?;
        if egress_binding_changed && let Some(ref audit) = self.audit {
            let _ = audit
                .success(
                    ACTOR_SYSTEM_ADMINISTRATOR,
                    None,
                    "collector.egress_binding_changed",
                    Some("collector_account"),
                    Some(&id.to_string()),
                    json!({
                        "fixed_egress_id": binding.fixed_egress_id.map(|e| e.to_string()),
                        "requires_relogin": binding.requires_relogin,
                    }),
                    now,
                )
                .await;
        }
        Ok(to_public(&account, Some(&binding)))
    }
}
