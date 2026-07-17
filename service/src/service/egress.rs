//! 出口与供应商管理服务（Admin CRUD + test）。

use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{Value, json};
use server_core::{AppError, Clock};
use uuid::Uuid;

use crate::adapters::proxy_provider::{
    DefaultProxyProviderAdapter, EgressTestResult, ProxyProviderAdapter,
};
use crate::adapters::secret_store::SecretStore;
use crate::audit::AuditWriter;
use crate::domain::egress::{
    EgressKind, EgressProfile, EgressProfilePublic, EgressPurpose, EgressStatus, ProxyProvider,
    ProxyProviderMode, ProxyProviderPublic, parse_purposes, validate_purposes_for_kind,
};
use crate::repository::EgressRepository;
use crate::service::AuthSession;

#[derive(Debug, Deserialize)]
pub struct CreateProxyProviderCommand {
    pub name: String,
    pub mode: ProxyProviderMode,
    pub api_base_url: Option<String>,
    /// 明文密钥：写入 SecretStore，库中只存 ref；响应不回显。
    pub auth_secret: Option<String>,
    #[serde(default)]
    pub request_template_json: Option<Value>,
    #[serde(default)]
    pub response_mapping_json: Option<Value>,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

#[derive(Debug, Deserialize)]
pub struct PatchProxyProviderCommand {
    pub name: Option<String>,
    pub mode: Option<ProxyProviderMode>,
    pub api_base_url: Option<String>,
    /// 若提供则轮换密钥引用。
    pub auth_secret: Option<String>,
    /// 显式清除 auth_secret_ref。
    #[serde(default)]
    pub clear_auth_secret: bool,
    pub request_template_json: Option<Value>,
    pub response_mapping_json: Option<Value>,
    pub enabled: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct CreateEgressProfileCommand {
    pub name: String,
    pub kind: EgressKind,
    pub provider_id: Option<Uuid>,
    /// 端点明文：写入 SecretStore。
    pub endpoint_secret: Option<String>,
    #[serde(default = "default_priority")]
    pub priority: i32,
    pub allowed_purposes: Vec<String>,
    #[serde(default = "default_active")]
    pub status: String,
}

#[derive(Debug, Deserialize)]
pub struct PatchEgressProfileCommand {
    pub name: Option<String>,
    pub kind: Option<EgressKind>,
    pub provider_id: Option<Uuid>,
    pub endpoint_secret: Option<String>,
    #[serde(default)]
    pub clear_endpoint_secret: bool,
    pub priority: Option<i32>,
    pub allowed_purposes: Option<Vec<String>>,
    pub status: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
pub struct TestEgressCommand {
    pub purpose: Option<String>,
}

fn default_true() -> bool {
    true
}
fn default_priority() -> i32 {
    100
}
fn default_active() -> String {
    "active".into()
}

/// 出口管理服务。
pub struct EgressService {
    repo: Arc<dyn EgressRepository>,
    secrets: Arc<dyn SecretStore>,
    audit: AuditWriter,
    clock: Arc<dyn Clock>,
    providers: Arc<dyn ProxyProviderAdapter>,
}

impl EgressService {
    pub fn new(
        repo: Arc<dyn EgressRepository>,
        secrets: Arc<dyn SecretStore>,
        audit: AuditWriter,
        clock: Arc<dyn Clock>,
    ) -> Self {
        let providers = Arc::new(DefaultProxyProviderAdapter::new(secrets.clone()));
        Self {
            repo,
            secrets,
            audit,
            clock,
            providers,
        }
    }

    pub fn with_provider_adapter(
        repo: Arc<dyn EgressRepository>,
        secrets: Arc<dyn SecretStore>,
        audit: AuditWriter,
        clock: Arc<dyn Clock>,
        providers: Arc<dyn ProxyProviderAdapter>,
    ) -> Self {
        Self {
            repo,
            secrets,
            audit,
            clock,
            providers,
        }
    }

    /// 解析一次登录链路使用的固定代理端点。
    ///
    /// 返回值只允许写入 SecretStore 中的 provider state，禁止进入响应、审计或日志。
    pub(crate) async fn resolve_login_proxy(
        &self,
        profile_id: Uuid,
    ) -> Result<Option<String>, AppError> {
        let profile = self
            .repo
            .get_egress_profile(profile_id)
            .await
            .map_err(|_| AppError::new("egress.repository_failed", "读取固定出口失败"))?
            .ok_or_else(|| AppError::not_found("egress.not_found", "固定出口不存在"))?;
        if profile.status != EgressStatus::Active {
            return Err(AppError::new("egress.unhealthy", "固定出口未启用"));
        }
        if !profile.kind.allows_purpose(EgressPurpose::Login)
            || !profile.allowed_purposes.contains(&EgressPurpose::Login)
        {
            return Err(AppError::new(
                "egress.purpose_mismatch",
                "固定出口未允许 login 用途",
            ));
        }
        match profile.kind {
            EgressKind::LocalDirect => Ok(None),
            EgressKind::Dynamic => Err(AppError::new(
                "egress.kind_mismatch",
                "扫码登录禁止使用动态出口",
            )),
            EgressKind::Fixed => {
                if let Some(provider_id) = profile.provider_id {
                    let provider = self
                        .repo
                        .get_proxy_provider(provider_id)
                        .await
                        .map_err(|_| {
                            AppError::new("egress.repository_failed", "读取出口供应商失败")
                        })?
                        .ok_or_else(|| {
                            AppError::not_found("egress.provider_not_found", "出口供应商不存在")
                        })?;
                    let allocated = self
                        .providers
                        .allocate(
                            &provider,
                            profile.endpoint_secret_ref.as_deref(),
                            EgressPurpose::Login,
                            DateTime::<Utc>::from(self.clock.now()),
                        )
                        .await
                        .map_err(|_| AppError::new("egress.provider_failed", "固定出口分配失败"))?;
                    Ok(Some(allocated.proxy_url))
                } else {
                    let secret_ref = profile.endpoint_secret_ref.as_deref().ok_or_else(|| {
                        AppError::new("egress.endpoint_not_configured", "固定出口端点未配置")
                    })?;
                    self.secrets
                        .get(secret_ref)
                        .await
                        .map_err(|_| {
                            AppError::new("secret_store.read_failed", "读取固定出口端点失败")
                        })?
                        .map(Some)
                        .ok_or_else(|| {
                            AppError::new("egress.endpoint_missing", "固定出口端点引用不存在")
                        })
                }
            }
        }
    }

    pub fn repo(&self) -> &Arc<dyn EgressRepository> {
        &self.repo
    }

    pub fn secrets(&self) -> &Arc<dyn SecretStore> {
        &self.secrets
    }

    fn now_utc(&self) -> DateTime<Utc> {
        system_time_to_utc(self.clock.now())
    }

    // --- proxy providers ---

    pub async fn list_proxy_providers(&self) -> Result<Vec<ProxyProviderPublic>, AppError> {
        let list = self
            .repo
            .list_proxy_providers()
            .await
            .map_err(AppError::internal)?;
        Ok(list.iter().map(ProxyProviderPublic::from).collect())
    }

    pub async fn create_proxy_provider(
        &self,
        auth: &AuthSession,
        cmd: CreateProxyProviderCommand,
    ) -> Result<ProxyProviderPublic, AppError> {
        if cmd.name.trim().is_empty() {
            return Err(AppError::new("egress.invalid_name", "供应商名称不能为空"));
        }
        let now = self.now_utc();
        let auth_secret_ref = if let Some(secret) = cmd.auth_secret.as_ref() {
            Some(self.secrets.put(secret).await.map_err(AppError::internal)?)
        } else {
            None
        };
        let p = ProxyProvider {
            id: Uuid::new_v4(),
            name: cmd.name.trim().to_string(),
            mode: cmd.mode,
            api_base_url: cmd.api_base_url,
            auth_secret_ref,
            request_template_json: cmd.request_template_json.unwrap_or_else(|| json!({})),
            response_mapping_json: cmd.response_mapping_json.unwrap_or_else(|| json!({})),
            enabled: cmd.enabled,
            created_at: now,
            updated_at: now,
        };
        self.repo
            .insert_proxy_provider(&p)
            .await
            .map_err(|e| map_conflict(e, "egress.provider_conflict"))?;
        self.audit_ok(
            auth,
            "egress.proxy_provider_created",
            "proxy_provider",
            &p.id,
            json!({
                "name": p.name,
                "mode": p.mode.as_str(),
                "has_auth_secret": p.auth_secret_ref.is_some(),
            }),
        )
        .await?;
        Ok(ProxyProviderPublic::from(&p))
    }

    pub async fn patch_proxy_provider(
        &self,
        auth: &AuthSession,
        id: Uuid,
        cmd: PatchProxyProviderCommand,
    ) -> Result<ProxyProviderPublic, AppError> {
        let mut p = self
            .repo
            .get_proxy_provider(id)
            .await
            .map_err(AppError::internal)?
            .ok_or_else(|| {
                AppError::not_found("egress.provider_not_found", format!("供应商不存在: {id}"))
            })?;
        if let Some(name) = cmd.name {
            if name.trim().is_empty() {
                return Err(AppError::new("egress.invalid_name", "供应商名称不能为空"));
            }
            p.name = name.trim().to_string();
        }
        if let Some(mode) = cmd.mode {
            p.mode = mode;
        }
        if let Some(url) = cmd.api_base_url {
            p.api_base_url = Some(url);
        }
        if cmd.clear_auth_secret {
            if let Some(old) = p.auth_secret_ref.take() {
                let _ = self.secrets.delete(&old).await;
            }
        } else if let Some(secret) = cmd.auth_secret {
            if let Some(old) = p.auth_secret_ref.take() {
                let _ = self.secrets.delete(&old).await;
            }
            p.auth_secret_ref = Some(
                self.secrets
                    .put(&secret)
                    .await
                    .map_err(AppError::internal)?,
            );
        }
        if let Some(v) = cmd.request_template_json {
            p.request_template_json = v;
        }
        if let Some(v) = cmd.response_mapping_json {
            p.response_mapping_json = v;
        }
        if let Some(e) = cmd.enabled {
            p.enabled = e;
        }
        p.updated_at = self.now_utc();
        self.repo
            .update_proxy_provider(&p)
            .await
            .map_err(|e| map_conflict(e, "egress.provider_conflict"))?;
        self.audit_ok(
            auth,
            "egress.proxy_provider_updated",
            "proxy_provider",
            &p.id,
            json!({
                "name": p.name,
                "has_auth_secret": p.auth_secret_ref.is_some(),
            }),
        )
        .await?;
        Ok(ProxyProviderPublic::from(&p))
    }

    // --- egress profiles ---

    pub async fn list_egress_profiles(&self) -> Result<Vec<EgressProfilePublic>, AppError> {
        let list = self
            .repo
            .list_egress_profiles()
            .await
            .map_err(AppError::internal)?;
        Ok(list.iter().map(EgressProfilePublic::from).collect())
    }

    /// 按 id 取出口（含内部字段，供 collector-login 校验用途）。
    pub async fn get_egress_profile(&self, id: Uuid) -> Result<Option<EgressProfile>, AppError> {
        self.repo
            .get_egress_profile(id)
            .await
            .map_err(AppError::internal)
    }

    pub async fn create_egress_profile(
        &self,
        auth: &AuthSession,
        cmd: CreateEgressProfileCommand,
    ) -> Result<EgressProfilePublic, AppError> {
        if cmd.name.trim().is_empty() {
            return Err(AppError::new("egress.invalid_name", "出口名称不能为空"));
        }
        let purposes = parse_purposes(&cmd.allowed_purposes)
            .map_err(|m| AppError::new("egress.invalid_purpose", m))?;
        validate_purposes_for_kind(cmd.kind, &purposes)
            .map_err(|m| AppError::new("egress.purpose_mismatch", m))?;
        self.validate_shape(cmd.kind, cmd.provider_id, cmd.endpoint_secret.is_some())?;

        if let Some(pid) = cmd.provider_id {
            self.ensure_provider_exists(pid).await?;
        }

        let now = self.now_utc();
        let endpoint_secret_ref = if let Some(secret) = cmd.endpoint_secret.as_ref() {
            Some(self.secrets.put(secret).await.map_err(AppError::internal)?)
        } else {
            None
        };

        if cmd.kind == EgressKind::LocalDirect
            && (endpoint_secret_ref.is_some() || cmd.provider_id.is_some())
        {
            return Err(AppError::new(
                "egress.invalid_local",
                "本机出口不能关联供应商或端点密钥",
            ));
        }

        let status = EgressStatus::parse(&cmd.status)
            .map_err(|m| AppError::new("egress.invalid_status", m))?;

        let p = EgressProfile {
            id: Uuid::new_v4(),
            name: cmd.name.trim().to_string(),
            kind: cmd.kind,
            provider_id: cmd.provider_id,
            endpoint_secret_ref,
            priority: cmd.priority,
            allowed_purposes: purposes,
            status,
            last_preflight_status: None,
            last_preflight_at: None,
            created_at: now,
            updated_at: now,
        };
        self.repo
            .insert_egress_profile(&p)
            .await
            .map_err(|e| map_conflict(e, "egress.profile_conflict"))?;
        self.audit_ok(
            auth,
            "egress.profile_created",
            "egress_profile",
            &p.id,
            json!({
                "name": p.name,
                "kind": p.kind.as_str(),
                "has_endpoint_secret": p.endpoint_secret_ref.is_some(),
            }),
        )
        .await?;
        Ok(EgressProfilePublic::from(&p))
    }

    pub async fn patch_egress_profile(
        &self,
        auth: &AuthSession,
        id: Uuid,
        cmd: PatchEgressProfileCommand,
    ) -> Result<EgressProfilePublic, AppError> {
        let mut p = self
            .repo
            .get_egress_profile(id)
            .await
            .map_err(AppError::internal)?
            .ok_or_else(|| AppError::not_found("egress.not_found", format!("出口不存在: {id}")))?;

        if let Some(name) = cmd.name {
            if name.trim().is_empty() {
                return Err(AppError::new("egress.invalid_name", "出口名称不能为空"));
            }
            p.name = name.trim().to_string();
        }
        if let Some(kind) = cmd.kind {
            p.kind = kind;
        }
        if let Some(pid) = cmd.provider_id {
            self.ensure_provider_exists(pid).await?;
            p.provider_id = Some(pid);
        }
        if cmd.clear_endpoint_secret {
            if let Some(old) = p.endpoint_secret_ref.take() {
                let _ = self.secrets.delete(&old).await;
            }
        } else if let Some(secret) = cmd.endpoint_secret {
            if let Some(old) = p.endpoint_secret_ref.take() {
                let _ = self.secrets.delete(&old).await;
            }
            p.endpoint_secret_ref = Some(
                self.secrets
                    .put(&secret)
                    .await
                    .map_err(AppError::internal)?,
            );
        }
        if let Some(pri) = cmd.priority {
            p.priority = pri;
        }
        if let Some(raw) = cmd.allowed_purposes {
            let purposes =
                parse_purposes(&raw).map_err(|m| AppError::new("egress.invalid_purpose", m))?;
            p.allowed_purposes = purposes;
        }
        if let Some(st) = cmd.status {
            p.status =
                EgressStatus::parse(&st).map_err(|m| AppError::new("egress.invalid_status", m))?;
        }

        validate_purposes_for_kind(p.kind, &p.allowed_purposes)
            .map_err(|m| AppError::new("egress.purpose_mismatch", m))?;
        if p.kind == EgressKind::LocalDirect
            && (p.provider_id.is_some() || p.endpoint_secret_ref.is_some())
        {
            return Err(AppError::new(
                "egress.invalid_local",
                "本机出口不能关联供应商或端点密钥",
            ));
        }

        p.updated_at = self.now_utc();
        self.repo
            .update_egress_profile(&p)
            .await
            .map_err(|e| map_conflict(e, "egress.profile_conflict"))?;
        self.audit_ok(
            auth,
            "egress.profile_updated",
            "egress_profile",
            &p.id,
            json!({
                "name": p.name,
                "kind": p.kind.as_str(),
                "has_endpoint_secret": p.endpoint_secret_ref.is_some(),
            }),
        )
        .await?;
        Ok(EgressProfilePublic::from(&p))
    }

    /// 测试出口连通性（fake：校验配置与密钥可读，不发起真实外网）。
    pub async fn test_egress_profile(
        &self,
        auth: &AuthSession,
        id: Uuid,
        cmd: TestEgressCommand,
    ) -> Result<EgressTestResult, AppError> {
        let started = Instant::now();
        let profile = self
            .repo
            .get_egress_profile(id)
            .await
            .map_err(AppError::internal)?
            .ok_or_else(|| AppError::not_found("egress.not_found", format!("出口不存在: {id}")))?;

        let purpose = if let Some(raw) = cmd.purpose.as_deref() {
            Some(
                EgressPurpose::parse(raw)
                    .map_err(|m| AppError::new("egress.invalid_purpose", m))?,
            )
        } else {
            profile.allowed_purposes.first().copied()
        };

        if let Some(purpose) = purpose
            && (!profile.kind.allows_purpose(purpose)
                || !profile.allowed_purposes.contains(&purpose))
        {
            let latency_ms = started.elapsed().as_millis() as u64;
            let result = EgressTestResult {
                ok: false,
                profile_id: id,
                kind: profile.kind.as_str().into(),
                purpose: Some(purpose.as_str().into()),
                latency_ms,
                message: "用途与出口种类不匹配".into(),
                error_code: Some("egress.purpose_mismatch".into()),
            };
            self.audit_ok(
                auth,
                "egress.profile_tested",
                "egress_profile",
                &id,
                json!({ "ok": false, "error_code": "egress.purpose_mismatch" }),
            )
            .await?;
            return Ok(result);
        }

        let result = match profile.kind {
            EgressKind::LocalDirect => EgressTestResult {
                ok: true,
                profile_id: id,
                kind: "local_direct".into(),
                purpose: purpose.map(|p| p.as_str().into()),
                latency_ms: started.elapsed().as_millis() as u64,
                message: "本机出口可用".into(),
                error_code: None,
            },
            EgressKind::Fixed | EgressKind::Dynamic => {
                let purpose = purpose.unwrap_or(EgressPurpose::ArticleDetail);
                match self.allocate_for_test(&profile, purpose).await {
                    Ok(_) => EgressTestResult {
                        ok: true,
                        profile_id: id,
                        kind: profile.kind.as_str().into(),
                        purpose: Some(purpose.as_str().into()),
                        latency_ms: started.elapsed().as_millis() as u64,
                        message: "出口配置与密钥引用校验通过".into(),
                        error_code: None,
                    },
                    Err(e) => EgressTestResult {
                        ok: false,
                        profile_id: id,
                        kind: profile.kind.as_str().into(),
                        purpose: Some(purpose.as_str().into()),
                        latency_ms: started.elapsed().as_millis() as u64,
                        message: e.message.clone(),
                        error_code: Some(e.code),
                    },
                }
            }
        };

        self.audit_ok(
            auth,
            "egress.profile_tested",
            "egress_profile",
            &id,
            json!({
                "ok": result.ok,
                "error_code": result.error_code,
                // 绝不写入密钥或完整代理 URL
            }),
        )
        .await?;
        Ok(result)
    }

    async fn allocate_for_test(
        &self,
        profile: &EgressProfile,
        purpose: EgressPurpose,
    ) -> Result<(), AppError> {
        let pid = profile
            .provider_id
            .ok_or_else(|| AppError::new("egress.provider_required", "出口需要关联供应商"))?;
        let provider = self
            .repo
            .get_proxy_provider(pid)
            .await
            .map_err(AppError::internal)?
            .ok_or_else(|| AppError::not_found("egress.provider_not_found", "供应商不存在"))?;
        self.providers
            .allocate(
                &provider,
                profile.endpoint_secret_ref.as_deref(),
                purpose,
                Utc::now(),
            )
            .await
            .map_err(|e| AppError::new("egress.provider_failed", e.to_string()))?;
        Ok(())
    }

    fn validate_shape(
        &self,
        kind: EgressKind,
        provider_id: Option<Uuid>,
        has_endpoint_secret: bool,
    ) -> Result<(), AppError> {
        if kind == EgressKind::LocalDirect && (provider_id.is_some() || has_endpoint_secret) {
            return Err(AppError::new(
                "egress.invalid_local",
                "本机出口不能关联供应商或端点密钥",
            ));
        }
        Ok(())
    }

    async fn ensure_provider_exists(&self, id: Uuid) -> Result<(), AppError> {
        self.repo
            .get_proxy_provider(id)
            .await
            .map_err(AppError::internal)?
            .ok_or_else(|| {
                AppError::not_found("egress.provider_not_found", format!("供应商不存在: {id}"))
            })?;
        Ok(())
    }

    async fn audit_ok(
        &self,
        auth: &AuthSession,
        action: &str,
        resource_type: &str,
        resource_id: &Uuid,
        details: Value,
    ) -> Result<(), AppError> {
        use crate::audit::ACTOR_SYSTEM_ADMINISTRATOR;
        self.audit
            .success(
                ACTOR_SYSTEM_ADMINISTRATOR,
                Some(&auth.username),
                action,
                Some(resource_type),
                Some(&resource_id.to_string()),
                details,
                self.now_utc(),
            )
            .await
            .map_err(AppError::internal)
    }
}

fn system_time_to_utc(t: SystemTime) -> DateTime<Utc> {
    let dur = t.duration_since(UNIX_EPOCH).unwrap_or_default();
    DateTime::<Utc>::from_timestamp(dur.as_secs() as i64, dur.subsec_nanos())
        .unwrap_or_else(|| DateTime::<Utc>::from_timestamp(0, 0).unwrap())
}

fn map_conflict(e: String, code: &str) -> AppError {
    if e.contains("already exists") || e.contains("conflict") || e.contains("duplicate") {
        AppError::new(code, e)
    } else {
        AppError::internal(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::secret_store::MemorySecretStore;
    use crate::audit::AuditWriter;
    use crate::domain::BootstrapCredentials;
    use crate::repository::{
        AdministratorRepository, AuditRepository, MemoryAdministratorStore, MemoryEgressStore,
        SettingsRepository,
    };
    use crate::service::administrator::{AdministratorService, LoginCommand};
    use server_core::FrozenClock;
    use std::time::Duration;

    async fn boot_admin() -> (AuthSession, Arc<MemoryAdministratorStore>, Arc<dyn Clock>) {
        let clock = Arc::new(FrozenClock::at_unix_millis(1_700_000_000_000));
        let store = Arc::new(MemoryAdministratorStore::new());
        let admin = AdministratorService::new(
            store.clone() as Arc<dyn AdministratorRepository>,
            store.clone() as Arc<dyn SettingsRepository>,
            AuditWriter::new(store.clone() as Arc<dyn AuditRepository>),
            clock.clone() as Arc<dyn Clock>,
            Duration::from_secs(3600),
        );
        admin
            .bootstrap(BootstrapCredentials {
                username: "admin".into(),
                password: "Bootstrap9!".into(),
            })
            .await
            .unwrap();
        let login = admin
            .login(LoginCommand {
                username: "admin".into(),
                password: "Bootstrap9!".into(),
                ip: None,
                user_agent: None,
            })
            .await
            .unwrap();
        let auth = admin
            .sessions()
            .authenticate_token(login.token.as_str())
            .await
            .unwrap();
        (auth, store, clock)
    }

    #[tokio::test]
    async fn create_provider_does_not_echo_secret() {
        let (auth, audit_store, clock) = boot_admin().await;
        let egress = Arc::new(MemoryEgressStore::with_local_direct());
        let secrets = Arc::new(MemorySecretStore::new());
        let svc = EgressService::new(
            egress as Arc<dyn EgressRepository>,
            secrets,
            AuditWriter::new(audit_store.clone() as Arc<dyn AuditRepository>),
            clock,
        );
        let pub_p = svc
            .create_proxy_provider(
                &auth,
                CreateProxyProviderCommand {
                    name: "gw".into(),
                    mode: ProxyProviderMode::Gateway,
                    api_base_url: Some("http://gw".into()),
                    auth_secret: Some("super-secret-token".into()),
                    request_template_json: None,
                    response_mapping_json: None,
                    enabled: true,
                },
            )
            .await
            .unwrap();
        let v = serde_json::to_value(&pub_p).unwrap();
        let s = v.to_string();
        assert!(!s.contains("super-secret-token"));
        assert!(v.get("auth_secret").is_none());
        assert!(v.get("auth_secret_ref").is_none());
        assert_eq!(v["has_auth_secret"], true);
    }

    #[tokio::test]
    async fn fixed_profile_rejects_detail_purposes() {
        let (auth, audit_store, clock) = boot_admin().await;
        let egress = Arc::new(MemoryEgressStore::with_local_direct());
        let secrets = Arc::new(MemorySecretStore::new());
        let svc = EgressService::new(
            egress as Arc<dyn EgressRepository>,
            secrets,
            AuditWriter::new(audit_store as Arc<dyn AuditRepository>),
            clock,
        );
        let err = svc
            .create_egress_profile(
                &auth,
                CreateEgressProfileCommand {
                    name: "bad".into(),
                    kind: EgressKind::Fixed,
                    provider_id: None,
                    endpoint_secret: None,
                    priority: 10,
                    allowed_purposes: vec!["article_detail".into()],
                    status: "active".into(),
                },
            )
            .await
            .unwrap_err();
        assert_eq!(err.code, "egress.purpose_mismatch");
    }

    #[tokio::test]
    async fn test_local_direct_ok() {
        let (auth, audit_store, clock) = boot_admin().await;
        let egress = Arc::new(MemoryEgressStore::with_local_direct());
        let secrets = Arc::new(MemorySecretStore::new());
        let svc = EgressService::new(
            egress as Arc<dyn EgressRepository>,
            secrets,
            AuditWriter::new(audit_store as Arc<dyn AuditRepository>),
            clock,
        );
        let r = svc
            .test_egress_profile(
                &auth,
                crate::domain::egress::LOCAL_DIRECT_PROFILE_ID,
                TestEgressCommand {
                    purpose: Some("login".into()),
                },
            )
            .await
            .unwrap();
        assert!(r.ok);
        assert!(r.error_code.is_none());
    }

    #[tokio::test]
    async fn login_proxy_is_resolved_once_from_secret_store() {
        let (auth, audit_store, clock) = boot_admin().await;
        let egress = Arc::new(MemoryEgressStore::with_local_direct());
        let secrets = Arc::new(MemorySecretStore::new());
        let svc = EgressService::new(
            egress as Arc<dyn EgressRepository>,
            secrets,
            AuditWriter::new(audit_store as Arc<dyn AuditRepository>),
            clock,
        );
        let profile = svc
            .create_egress_profile(
                &auth,
                CreateEgressProfileCommand {
                    name: "login-fixed".into(),
                    kind: EgressKind::Fixed,
                    provider_id: None,
                    endpoint_secret: Some("http://user:password@127.0.0.1:8080".into()),
                    priority: 1,
                    allowed_purposes: vec!["login".into(), "discover".into()],
                    status: "active".into(),
                },
            )
            .await
            .unwrap();

        assert_eq!(
            svc.resolve_login_proxy(profile.id)
                .await
                .unwrap()
                .as_deref(),
            Some("http://user:password@127.0.0.1:8080")
        );
    }
}
