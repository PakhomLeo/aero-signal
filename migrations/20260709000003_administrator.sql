-- M03 administrator：唯一管理员、会话、系统设置、审计。
-- 不写入任何明文初始密码；bootstrap 由部署环境变量完成。

CREATE TABLE IF NOT EXISTS system_administrator (
    singleton BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (singleton),
    username TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    must_change_password BOOLEAN NOT NULL DEFAULT TRUE,
    password_changed_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS admin_sessions (
    id UUID PRIMARY KEY,
    session_hash TEXT NOT NULL UNIQUE,
    expires_at TIMESTAMPTZ NOT NULL,
    revoked_at TIMESTAMPTZ,
    ip_hash TEXT,
    user_agent_hash TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_admin_sessions_expires
    ON admin_sessions (expires_at)
    WHERE revoked_at IS NULL;

-- 唯一系统设置行：调度/时区/工作窗口/图片与 OCR 阈值。
CREATE TABLE IF NOT EXISTS system_settings (
    singleton BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (singleton),
    timezone TEXT NOT NULL DEFAULT 'Asia/Shanghai',
    preflight_time TEXT NOT NULL DEFAULT '07:30',
    plan_start_time TEXT NOT NULL DEFAULT '08:00',
    working_windows JSONB NOT NULL DEFAULT '[{"start":"09:00","end":"18:00"}]'::jsonb,
    image_min_side_px INTEGER NOT NULL DEFAULT 80,
    image_min_area_px INTEGER NOT NULL DEFAULT 14400,
    ocr_min_confidence NUMERIC(5, 4) NOT NULL DEFAULT 0.7000,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

INSERT INTO system_settings (singleton)
VALUES (TRUE)
ON CONFLICT (singleton) DO NOTHING;

-- 审计：不记录明文密码/凭据。
CREATE TABLE IF NOT EXISTS audit_logs (
    id UUID PRIMARY KEY,
    actor_type TEXT NOT NULL
        CHECK (actor_type IN (
            'system_administrator', 'api_token', 'system', 'worker'
        )),
    actor_id TEXT,
    action TEXT NOT NULL,
    resource_type TEXT,
    resource_id TEXT,
    result TEXT NOT NULL DEFAULT 'success'
        CHECK (result IN ('success', 'failure')),
    details_json JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_audit_logs_created
    ON audit_logs (created_at DESC);

CREATE INDEX IF NOT EXISTS idx_audit_logs_action
    ON audit_logs (action, created_at DESC);
