-- M07 source：在已有 source_accounts 桩上补齐标识、验证 run 与抓取规则。
-- 000006 可能已创建 source_accounts；本迁移仅 ALTER/补表，不破坏既有结构。

-- source_accounts：确保列齐全（IF NOT EXISTS 风格用 DO 块兼容）
DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM information_schema.columns
        WHERE table_name = 'source_accounts' AND column_name = 'preferred_connector'
    ) THEN
        ALTER TABLE source_accounts ADD COLUMN preferred_connector TEXT;
    END IF;
    IF NOT EXISTS (
        SELECT 1 FROM information_schema.columns
        WHERE table_name = 'source_accounts' AND column_name = 'added_at'
    ) THEN
        ALTER TABLE source_accounts ADD COLUMN added_at TIMESTAMPTZ NOT NULL DEFAULT now();
    END IF;
    IF NOT EXISTS (
        SELECT 1 FROM information_schema.columns
        WHERE table_name = 'source_accounts' AND column_name = 'last_fetch_at'
    ) THEN
        ALTER TABLE source_accounts ADD COLUMN last_fetch_at TIMESTAMPTZ;
    END IF;
    IF NOT EXISTS (
        SELECT 1 FROM information_schema.columns
        WHERE table_name = 'source_accounts' AND column_name = 'created_at'
    ) THEN
        ALTER TABLE source_accounts ADD COLUMN created_at TIMESTAMPTZ NOT NULL DEFAULT now();
    END IF;
    IF NOT EXISTS (
        SELECT 1 FROM information_schema.columns
        WHERE table_name = 'source_accounts' AND column_name = 'updated_at'
    ) THEN
        ALTER TABLE source_accounts ADD COLUMN updated_at TIMESTAMPTZ NOT NULL DEFAULT now();
    END IF;
END $$;

CREATE TABLE IF NOT EXISTS source_accounts (
    id UUID PRIMARY KEY,
    display_name TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'verifying'
        CHECK (status IN (
            'verifying',
            'pending_configuration',
            'active',
            'paused',
            'needs_action',
            'archived'
        )),
    preferred_connector TEXT,
    added_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_fetch_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS source_account_ids (
    id UUID PRIMARY KEY,
    source_account_id UUID NOT NULL
        REFERENCES source_accounts(id) ON DELETE CASCADE,
    id_type TEXT NOT NULL
        CHECK (id_type IN ('biz', 'fakeid', 'alias', 'weread_mp_id')),
    id_value TEXT NOT NULL,
    confidence TEXT NOT NULL
        CHECK (confidence IN ('strong', 'weak')),
    UNIQUE (id_type, id_value)
);

CREATE INDEX IF NOT EXISTS idx_source_account_ids_source
    ON source_account_ids (source_account_id);

CREATE TABLE IF NOT EXISTS source_validation_runs (
    id UUID PRIMARY KEY,
    input_url TEXT NOT NULL,
    input_hash TEXT NOT NULL UNIQUE,
    parsed_source_json JSONB NOT NULL,
    connector_account_id UUID
        REFERENCES collector_accounts(id) ON DELETE SET NULL,
    status TEXT NOT NULL
        CHECK (status IN ('running', 'succeeded', 'failed', 'expired')),
    candidate_count SMALLINT NOT NULL DEFAULT 0
        CHECK (candidate_count BETWEEN 0 AND 3),
    successful_count SMALLINT NOT NULL DEFAULT 0
        CHECK (successful_count BETWEEN 0 AND 3),
    -- 扩展：可用候选数、缺少原因、候选明细、关联 draft source
    available_count SMALLINT NOT NULL DEFAULT 0,
    missing_reason TEXT,
    candidates_json JSONB NOT NULL DEFAULT '[]'::jsonb,
    source_account_id UUID
        REFERENCES source_accounts(id) ON DELETE SET NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    completed_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_source_validation_runs_expires
    ON source_validation_runs (expires_at)
    WHERE status IN ('running', 'succeeded');

CREATE TABLE IF NOT EXISTS account_fetch_rules (
    source_account_id UUID PRIMARY KEY
        REFERENCES source_accounts(id) ON DELETE CASCADE,
    base_level TEXT NOT NULL
        CHECK (base_level IN ('LO0', 'LO1', 'L1', 'L2', 'L3', 'L4')),
    connector_override TEXT,
    enabled BOOLEAN NOT NULL DEFAULT true,
    first_heartbeat_at TIMESTAMPTZ NOT NULL,
    next_heartbeat_at TIMESTAMPTZ NOT NULL,
    working_windows JSONB NOT NULL,
    interval_override_hours INT,
    consecutive_empty_fetches INT NOT NULL DEFAULT 0,
    promotion_until TIMESTAMPTZ,
    manual_next_heartbeat_at TIMESTAMPTZ,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_source_due
    ON account_fetch_rules (next_heartbeat_at)
    WHERE enabled;
