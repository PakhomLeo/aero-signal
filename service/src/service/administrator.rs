//! 唯一管理员服务：bootstrap、登录、会话、改密/改账号、调度设置。

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Utc};
use serde_json::json;
use server_core::{AppError, Clock};
use uuid::Uuid;

use crate::audit::AuditWriter;
use crate::domain::administrator::AdministratorPublic;
use crate::domain::session::hash_client_hint;
use crate::domain::{
    Administrator, BootstrapCredentials, PasswordHasher, ScheduleSettings, SessionToken,
    generate_session_token, hash_session_token,
};
use crate::repository::administrator::{AdminSessionRecord, AdministratorRepository};
use crate::repository::settings::SettingsRepository;

/// 默认会话有效期：12 小时。
pub const DEFAULT_SESSION_TTL: Duration = Duration::from_secs(12 * 60 * 60);

/// 已认证会话上下文（注入到受保护路由）。
#[derive(Debug, Clone)]
pub struct AuthSession {
    pub session_id: Uuid,
    /// 会话 token 的 SHA-256 hex（用于 ticket 归属校验；不回显给客户端）。
    pub session_hash: String,
    pub username: String,
    pub must_change_password: bool,
}

/// 登录结果。
#[derive(Debug, Clone)]
pub struct LoginResult {
    pub token: SessionToken,
    pub expires_at: DateTime<Utc>,
    pub admin: AdministratorPublic,
}

/// Bootstrap 结果。
#[derive(Debug, Clone)]
pub struct BootstrapResult {
    pub username: String,
    pub must_change_password: bool,
}

#[derive(Debug, Clone)]
pub struct LoginCommand {
    pub username: String,
    pub password: String,
    pub ip: Option<String>,
    pub user_agent: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ChangePasswordCommand {
    pub current_password: String,
    pub new_password: String,
}

#[derive(Debug, Clone, Default)]
pub struct PatchMeCommand {
    pub username: Option<String>,
    pub current_password: Option<String>,
    pub new_password: Option<String>,
}

/// Session 校验与撤销。
pub struct SessionService {
    repo: Arc<dyn AdministratorRepository>,
    clock: Arc<dyn Clock>,
    session_ttl: Duration,
}

impl SessionService {
    pub fn new(
        repo: Arc<dyn AdministratorRepository>,
        clock: Arc<dyn Clock>,
        session_ttl: Duration,
    ) -> Self {
        Self {
            repo,
            clock,
            session_ttl,
        }
    }

    pub fn now_utc(&self) -> DateTime<Utc> {
        system_time_to_utc(self.clock.now())
    }

    pub async fn create_session(
        &self,
        ip: Option<&str>,
        user_agent: Option<&str>,
    ) -> Result<(SessionToken, AdminSessionRecord), AppError> {
        let token = generate_session_token();
        let now = self.now_utc();
        let expires_at = now
            + chrono::Duration::from_std(self.session_ttl).unwrap_or(chrono::Duration::hours(12));
        let record = AdminSessionRecord {
            id: Uuid::new_v4(),
            session_hash: hash_session_token(token.as_str()),
            expires_at,
            ip_hash: ip.map(hash_client_hint),
            user_agent_hash: user_agent.map(hash_client_hint),
            created_at: now,
        };
        self.repo
            .insert_session(&record)
            .await
            .map_err(AppError::internal)?;
        Ok((token, record))
    }

    pub async fn authenticate_token(&self, raw_token: &str) -> Result<AuthSession, AppError> {
        let hash = hash_session_token(raw_token);
        let session = self
            .repo
            .find_session_by_hash(&hash)
            .await
            .map_err(AppError::internal)?
            .ok_or_else(|| AppError::unauthorized("无效或已过期的会话"))?;

        let now = self.now_utc();
        if !session.is_active(now) {
            return Err(AppError::unauthorized("无效或已过期的会话"));
        }

        let admin = self
            .repo
            .get_administrator()
            .await
            .map_err(AppError::internal)?
            .ok_or_else(|| AppError::unauthorized("管理员未初始化"))?;

        Ok(AuthSession {
            session_id: session.id,
            session_hash: hash,
            username: admin.username,
            must_change_password: admin.must_change_password,
        })
    }

    pub async fn revoke(&self, session_id: Uuid) -> Result<(), AppError> {
        let now = self.now_utc();
        self.repo
            .revoke_session(session_id, now)
            .await
            .map_err(AppError::internal)
    }
}

/// 管理员应用服务。
pub struct AdministratorService {
    repo: Arc<dyn AdministratorRepository>,
    settings: Arc<dyn SettingsRepository>,
    audit: AuditWriter,
    hasher: PasswordHasher,
    sessions: SessionService,
    clock: Arc<dyn Clock>,
}

impl AdministratorService {
    pub fn new(
        repo: Arc<dyn AdministratorRepository>,
        settings: Arc<dyn SettingsRepository>,
        audit: AuditWriter,
        clock: Arc<dyn Clock>,
        session_ttl: Duration,
    ) -> Self {
        let sessions = SessionService::new(repo.clone(), clock.clone(), session_ttl);
        Self {
            repo,
            settings,
            audit,
            hasher: PasswordHasher::default(),
            sessions,
            clock,
        }
    }

    pub fn sessions(&self) -> &SessionService {
        &self.sessions
    }

    pub fn now_utc(&self) -> DateTime<Utc> {
        system_time_to_utc(self.clock.now())
    }

    /// 从环境变量读取 bootstrap 凭据；缺失则返回 None。
    pub fn bootstrap_credentials_from_env() -> Option<BootstrapCredentials> {
        let username = std::env::var("SIGNAL_BOOTSTRAP_USERNAME").ok()?;
        let password = std::env::var("SIGNAL_BOOTSTRAP_PASSWORD").ok()?;
        if username.trim().is_empty() || password.is_empty() {
            return None;
        }
        Some(BootstrapCredentials { username, password })
    }

    /// 启动时自动 bootstrap（若环境变量齐全且尚未存在管理员）。
    pub async fn try_bootstrap_from_env(&self) -> Result<Option<BootstrapResult>, AppError> {
        let Some(creds) = Self::bootstrap_credentials_from_env() else {
            return Ok(None);
        };
        match self.bootstrap(creds).await {
            Ok(r) => {
                tracing::info!(
                    username = %r.username,
                    "管理员 bootstrap 完成（must_change_password=true）"
                );
                Ok(Some(r))
            }
            Err(e) if e.code == "admin.already_bootstrapped" => {
                tracing::info!("管理员已存在，跳过 bootstrap");
                Ok(None)
            }
            Err(e) => Err(e),
        }
    }

    pub async fn bootstrap(
        &self,
        creds: BootstrapCredentials,
    ) -> Result<BootstrapResult, AppError> {
        if self
            .repo
            .get_administrator()
            .await
            .map_err(AppError::internal)?
            .is_some()
        {
            return Err(AppError::new(
                "admin.already_bootstrapped",
                "管理员已存在，拒绝重复 bootstrap",
            )
            .with_status(axum::http::StatusCode::CONFLICT));
        }

        let username = creds.username.trim().to_string();
        if username.is_empty() {
            return Err(AppError::new("admin.invalid_username", "用户名不能为空"));
        }

        let password_hash = self.hasher.hash_password(&creds.password).map_err(|e| {
            AppError::new("admin.weak_password", e.to_string())
                .with_status(axum::http::StatusCode::BAD_REQUEST)
        })?;

        // 丢弃明文：creds 在此之后不应再被使用。
        drop(creds);

        let now = self.now_utc();
        let admin = Administrator {
            username: username.clone(),
            password_hash,
            must_change_password: true,
            password_changed_at: None,
            created_at: now,
            updated_at: now,
        };

        self.repo.insert_administrator(&admin).await.map_err(|e| {
            if e.contains("already exists") {
                AppError::new(
                    "admin.already_bootstrapped",
                    "管理员已存在，拒绝重复 bootstrap",
                )
                .with_status(axum::http::StatusCode::CONFLICT)
            } else {
                AppError::internal(e)
            }
        })?;

        self.write_audit(
            "system",
            None,
            "admin.bootstrap",
            "system_administrator",
            Some(&username),
            "success",
            json!({ "must_change_password": true }),
        )
        .await?;

        Ok(BootstrapResult {
            username,
            must_change_password: true,
        })
    }

    pub async fn login(&self, cmd: LoginCommand) -> Result<LoginResult, AppError> {
        let admin = self
            .repo
            .get_administrator()
            .await
            .map_err(AppError::internal)?
            .ok_or_else(|| {
                AppError::new("admin.not_bootstrapped", "管理员尚未初始化")
                    .with_status(axum::http::StatusCode::SERVICE_UNAVAILABLE)
            })?;

        if admin.username != cmd.username
            || !self
                .hasher
                .verify_password(&cmd.password, &admin.password_hash)
        {
            self.write_audit(
                "system",
                None,
                "admin.login_failed",
                "system_administrator",
                None,
                "failure",
                json!({ "reason": "invalid_credentials" }),
            )
            .await
            .ok();
            return Err(AppError::unauthorized("用户名或密码错误"));
        }

        let (token, record) = self
            .sessions
            .create_session(cmd.ip.as_deref(), cmd.user_agent.as_deref())
            .await?;

        self.write_audit(
            "system_administrator",
            Some(&admin.username),
            "admin.login",
            "admin_session",
            Some(&record.id.to_string()),
            "success",
            json!({ "must_change_password": admin.must_change_password }),
        )
        .await?;

        Ok(LoginResult {
            token,
            expires_at: record.expires_at,
            admin: admin.public_view(),
        })
    }

    pub async fn logout(&self, session_id: Uuid) -> Result<(), AppError> {
        self.sessions.revoke(session_id).await?;
        self.write_audit(
            "system_administrator",
            None,
            "admin.logout",
            "admin_session",
            Some(&session_id.to_string()),
            "success",
            json!({}),
        )
        .await?;
        Ok(())
    }

    pub async fn get_me(&self) -> Result<AdministratorPublic, AppError> {
        let admin = self.require_admin().await?;
        Ok(admin.public_view())
    }

    pub async fn patch_me(
        &self,
        auth: &AuthSession,
        cmd: PatchMeCommand,
    ) -> Result<AdministratorPublic, AppError> {
        let changing_password = cmd.new_password.is_some();
        let changing_username = cmd
            .username
            .as_ref()
            .map(|u| u.trim() != auth.username)
            .unwrap_or(false);

        if auth.must_change_password && !changing_password {
            return Err(
                AppError::new("admin.must_change_password", "首次登录必须先修改密码")
                    .with_status(axum::http::StatusCode::FORBIDDEN),
            );
        }

        if !changing_password && !changing_username {
            return self.get_me().await;
        }

        let mut admin = self.require_admin().await?;
        let now = self.now_utc();

        if changing_password {
            let current = cmd.current_password.as_deref().unwrap_or("");
            let new_password = cmd.new_password.as_deref().unwrap_or("");
            if !self.hasher.verify_password(current, &admin.password_hash) {
                return Err(AppError::unauthorized("当前密码不正确"));
            }
            let new_hash = self
                .hasher
                .hash_password(new_password)
                .map_err(|e| AppError::new("admin.weak_password", e.to_string()))?;
            admin.password_hash = new_hash;
            admin.must_change_password = false;
            admin.password_changed_at = Some(now);
            admin.updated_at = now;

            self.repo
                .update_administrator(&admin)
                .await
                .map_err(AppError::internal)?;

            let revoked = self
                .repo
                .revoke_other_sessions(auth.session_id, now)
                .await
                .map_err(AppError::internal)?;

            self.write_audit(
                "system_administrator",
                Some(&admin.username),
                "admin.password_changed",
                "system_administrator",
                Some(&admin.username),
                "success",
                json!({ "revoked_other_sessions": revoked }),
            )
            .await?;
        }

        if changing_username {
            let new_username = cmd
                .username
                .as_ref()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .ok_or_else(|| AppError::new("admin.invalid_username", "用户名不能为空"))?;

            // 改账号也要求验证当前密码（若未在同请求中改密）。
            if !changing_password {
                let current = cmd.current_password.as_deref().unwrap_or("");
                if !self.hasher.verify_password(current, &admin.password_hash) {
                    return Err(AppError::unauthorized("当前密码不正确"));
                }
            }

            let old = admin.username.clone();
            admin.username = new_username;
            admin.updated_at = now;
            self.repo
                .update_administrator(&admin)
                .await
                .map_err(AppError::internal)?;

            self.write_audit(
                "system_administrator",
                Some(&admin.username),
                "admin.username_changed",
                "system_administrator",
                Some(&admin.username),
                "success",
                json!({ "from": old, "to": admin.username }),
            )
            .await?;
        }

        Ok(admin.public_view())
    }

    pub async fn get_schedule(&self) -> Result<ScheduleSettings, AppError> {
        self.settings
            .get_schedule()
            .await
            .map_err(AppError::internal)
    }

    pub async fn update_schedule(
        &self,
        auth: &AuthSession,
        settings: ScheduleSettings,
    ) -> Result<ScheduleSettings, AppError> {
        self.ensure_password_changed(auth)?;
        settings
            .validate()
            .map_err(|m| AppError::new("admin.invalid_schedule", m))?;
        self.settings
            .update_schedule(&settings)
            .await
            .map_err(AppError::internal)?;
        self.write_audit(
            "system_administrator",
            Some(&auth.username),
            "admin.schedule_updated",
            "system_settings",
            Some("schedule"),
            "success",
            json!({
                "timezone": settings.timezone,
                "preflight_time": settings.preflight_time,
                "plan_start_time": settings.plan_start_time,
            }),
        )
        .await?;
        Ok(settings)
    }

    /// 受保护路由：强制改密门禁（允许 /me PATCH 改密本身）。
    pub fn ensure_password_changed(&self, auth: &AuthSession) -> Result<(), AppError> {
        if auth.must_change_password {
            return Err(
                AppError::new("admin.must_change_password", "首次登录必须先修改密码")
                    .with_status(axum::http::StatusCode::FORBIDDEN),
            );
        }
        Ok(())
    }

    async fn require_admin(&self) -> Result<Administrator, AppError> {
        self.repo
            .get_administrator()
            .await
            .map_err(AppError::internal)?
            .ok_or_else(|| {
                AppError::new("admin.not_bootstrapped", "管理员尚未初始化")
                    .with_status(axum::http::StatusCode::SERVICE_UNAVAILABLE)
            })
    }

    #[allow(clippy::too_many_arguments)]
    async fn write_audit(
        &self,
        actor_type: &str,
        actor_id: Option<&str>,
        action: &str,
        resource_type: &str,
        resource_id: Option<&str>,
        result: &str,
        details: serde_json::Value,
    ) -> Result<(), AppError> {
        self.audit
            .write(
                actor_type,
                actor_id,
                action,
                Some(resource_type),
                resource_id,
                result,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::AuditWriter;
    use crate::repository::{AuditRepository, MemoryAdministratorStore};
    use server_core::FrozenClock;

    fn build_service(
        clock: Arc<FrozenClock>,
    ) -> (AdministratorService, Arc<MemoryAdministratorStore>) {
        let store = Arc::new(MemoryAdministratorStore::new());
        let svc = AdministratorService::new(
            store.clone() as Arc<dyn AdministratorRepository>,
            store.clone() as Arc<dyn SettingsRepository>,
            AuditWriter::new(store.clone() as Arc<dyn AuditRepository>),
            clock,
            Duration::from_secs(3600),
        );
        (svc, store)
    }

    async fn bootstrapped() -> (
        AdministratorService,
        Arc<MemoryAdministratorStore>,
        AuthSession,
        String,
    ) {
        let clock = Arc::new(FrozenClock::at_unix_millis(1_700_000_000_000));
        let (svc, store) = build_service(clock);

        svc.bootstrap(BootstrapCredentials {
            username: "admin".into(),
            password: "Bootstrap9!".into(),
        })
        .await
        .unwrap();

        let login = svc
            .login(LoginCommand {
                username: "admin".into(),
                password: "Bootstrap9!".into(),
                ip: None,
                user_agent: None,
            })
            .await
            .unwrap();
        let token = login.token.as_str().to_string();
        let auth = svc.sessions().authenticate_token(&token).await.unwrap();
        assert!(auth.must_change_password);
        (svc, store, auth, token)
    }

    #[tokio::test]
    async fn bootstrap_twice_fails() {
        let clock = Arc::new(FrozenClock::at_unix_millis(1_700_000_000_000));
        let (svc, _) = build_service(clock);
        svc.bootstrap(BootstrapCredentials {
            username: "admin".into(),
            password: "Bootstrap9!".into(),
        })
        .await
        .unwrap();
        let err = svc
            .bootstrap(BootstrapCredentials {
                username: "admin2".into(),
                password: "Bootstrap9!".into(),
            })
            .await
            .unwrap_err();
        assert_eq!(err.code, "admin.already_bootstrapped");
    }

    #[tokio::test]
    async fn weak_password_rejected_on_bootstrap() {
        let clock = Arc::new(FrozenClock::at_unix_millis(1_700_000_000_000));
        let (svc, _) = build_service(clock);
        let err = svc
            .bootstrap(BootstrapCredentials {
                username: "admin".into(),
                password: "short".into(),
            })
            .await
            .unwrap_err();
        assert_eq!(err.code, "admin.weak_password");
    }

    #[tokio::test]
    async fn change_password_revokes_other_sessions() {
        let (svc, store, auth, current_token) = bootstrapped().await;

        let other = svc
            .login(LoginCommand {
                username: "admin".into(),
                password: "Bootstrap9!".into(),
                ip: None,
                user_agent: None,
            })
            .await
            .unwrap();
        let other_token = other.token.into_inner();

        svc.patch_me(
            &auth,
            PatchMeCommand {
                username: None,
                current_password: Some("Bootstrap9!".into()),
                new_password: Some("NewSecure1!".into()),
            },
        )
        .await
        .unwrap();

        let err = svc
            .sessions()
            .authenticate_token(&other_token)
            .await
            .unwrap_err();
        assert_eq!(err.code, "auth.unauthorized");

        let still = svc
            .sessions()
            .authenticate_token(&current_token)
            .await
            .unwrap();
        assert!(!still.must_change_password);

        let audits = store.audits().await;
        assert!(audits.iter().any(|a| a.action == "admin.password_changed"));
        for a in &audits {
            let s = a.details_json.to_string();
            assert!(!s.contains("Bootstrap9"));
            assert!(!s.contains("NewSecure1"));
        }
    }

    #[tokio::test]
    async fn must_change_password_blocks_schedule() {
        let (svc, _, auth, _) = bootstrapped().await;
        let err = svc
            .update_schedule(&auth, ScheduleSettings::default())
            .await
            .unwrap_err();
        assert_eq!(err.code, "admin.must_change_password");
    }

    #[tokio::test]
    async fn session_hash_not_echoed_in_login_admin() {
        let (svc, _, _, _) = bootstrapped().await;
        let login = svc
            .login(LoginCommand {
                username: "admin".into(),
                password: "Bootstrap9!".into(),
                ip: None,
                user_agent: None,
            })
            .await
            .unwrap();
        let public = serde_json::to_value(login.admin).unwrap();
        assert!(public.get("password_hash").is_none());
        assert!(public.get("session_hash").is_none());
        let token = login.token.into_inner();
        assert!(!token.is_empty());
        assert_ne!(hash_session_token(&token), token);
    }

    #[tokio::test]
    async fn bootstrap_credentials_debug_redacts_password() {
        let c = BootstrapCredentials {
            username: "u".into(),
            password: "SuperSecret9!".into(),
        };
        let s = format!("{c:?}");
        assert!(s.contains("[redacted]"));
        assert!(!s.contains("SuperSecret9"));
    }
}
