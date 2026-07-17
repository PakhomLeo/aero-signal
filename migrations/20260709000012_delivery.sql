-- M13 delivery：分组、API Token、调用日志。
-- delivery_outbox / article_change_events 已在 M10。

CREATE TABLE IF NOT EXISTS delivery_groups (
    id UUID PRIMARY KEY,
    name TEXT NOT NULL,
    description TEXT,
    status TEXT NOT NULL DEFAULT 'active'
        CHECK (status IN ('active', 'disabled')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS delivery_group_sources (
    delivery_group_id UUID NOT NULL
        REFERENCES delivery_groups(id) ON DELETE CASCADE,
    source_account_id UUID NOT NULL
        REFERENCES source_accounts(id) ON DELETE RESTRICT,
    bound_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (delivery_group_id, source_account_id)
);

CREATE INDEX IF NOT EXISTS idx_delivery_group_sources_source
    ON delivery_group_sources (source_account_id);

CREATE TABLE IF NOT EXISTS api_tokens (
    id UUID PRIMARY KEY,
    delivery_group_id UUID NOT NULL
        REFERENCES delivery_groups(id) ON DELETE RESTRICT,
    name TEXT NOT NULL,
    token_prefix TEXT NOT NULL,
    token_hash TEXT NOT NULL UNIQUE,
    scopes TEXT[] NOT NULL,
    ip_allowlist TEXT[] NOT NULL DEFAULT '{}',
    expires_at TIMESTAMPTZ,
    revoked_at TIMESTAMPTZ,
    last_used_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_api_tokens_group
    ON api_tokens (delivery_group_id);

CREATE INDEX IF NOT EXISTS idx_api_tokens_prefix
    ON api_tokens (token_prefix);

CREATE TABLE IF NOT EXISTS api_call_logs (
    id UUID PRIMARY KEY,
    api_token_id UUID NOT NULL
        REFERENCES api_tokens(id) ON DELETE CASCADE,
    delivery_group_id UUID NOT NULL
        REFERENCES delivery_groups(id) ON DELETE CASCADE,
    method TEXT NOT NULL,
    path TEXT NOT NULL,
    scope_used TEXT,
    status_code INT NOT NULL,
    duration_ms INT NOT NULL,
    redaction_count INT NOT NULL DEFAULT 0,
    client_ip_hash TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_api_call_logs_token_time
    ON api_call_logs (api_token_id, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_api_call_logs_created
    ON api_call_logs (created_at);
