-- M05 collector-login：采集账号、登录会话、固定出口绑定。
-- 敏感凭据仅存 secret_ref；不进普通 JSON 列。

CREATE TABLE IF NOT EXISTS collector_accounts (
    id UUID PRIMARY KEY,
    connector TEXT NOT NULL
        CHECK (connector IN ('mp_admin', 'weread', 'fake')),
    display_name TEXT NOT NULL,
    external_account_ref TEXT,
    status TEXT NOT NULL
        CHECK (status IN ('active', 'needs_relogin', 'expired', 'disabled', 'failed')),
    capability_json JSONB NOT NULL DEFAULT '{}'::jsonb,
    last_probe_at TIMESTAMPTZ,
    last_success_at TIMESTAMPTZ,
    last_failure_at TIMESTAMPTZ,
    last_error_code TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_collector_accounts_connector_status
    ON collector_accounts (connector, status);

CREATE TABLE IF NOT EXISTS collector_login_sessions (
    id UUID PRIMARY KEY,
    collector_account_id UUID NOT NULL
        REFERENCES collector_accounts(id) ON DELETE CASCADE,
    credential_secret_ref TEXT NOT NULL,
    browser_profile_ref TEXT,
    expires_at TIMESTAMPTZ,
    confirmed_at TIMESTAMPTZ NOT NULL,
    revoked_at TIMESTAMPTZ,
    UNIQUE (collector_account_id, revoked_at)
);

CREATE INDEX IF NOT EXISTS idx_collector_login_sessions_account
    ON collector_login_sessions (collector_account_id);

CREATE TABLE IF NOT EXISTS collector_egress_bindings (
    collector_account_id UUID PRIMARY KEY
        REFERENCES collector_accounts(id) ON DELETE CASCADE,
    fixed_egress_id UUID
        REFERENCES egress_profiles(id) ON DELETE RESTRICT,
    bound_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    changed_at TIMESTAMPTZ,
    requires_relogin BOOLEAN NOT NULL DEFAULT FALSE
);

CREATE INDEX IF NOT EXISTS idx_collector_egress_bindings_egress
    ON collector_egress_bindings (fixed_egress_id)
    WHERE fixed_egress_id IS NOT NULL;
