-- M04 egress：代理供应商与出口配置；密钥仅存引用；默认本机出口种子。

CREATE TABLE IF NOT EXISTS proxy_providers (
    id UUID PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    mode TEXT NOT NULL
        CHECK (mode IN ('static_endpoint', 'gateway', 'vendor_api')),
    api_base_url TEXT,
    auth_secret_ref TEXT,
    request_template_json JSONB NOT NULL DEFAULT '{}'::jsonb,
    response_mapping_json JSONB NOT NULL DEFAULT '{}'::jsonb,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS egress_profiles (
    id UUID PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    kind TEXT NOT NULL
        CHECK (kind IN ('local_direct', 'fixed', 'dynamic')),
    provider_id UUID REFERENCES proxy_providers(id) ON DELETE RESTRICT,
    endpoint_secret_ref TEXT,
    priority INT NOT NULL DEFAULT 100,
    allowed_purposes TEXT[] NOT NULL,
    status TEXT NOT NULL DEFAULT 'active',
    last_preflight_status TEXT,
    last_preflight_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- 用途枚举：login / discover / article_detail / asset_download
    CONSTRAINT egress_profiles_purposes_valid CHECK (
        cardinality(allowed_purposes) >= 1
        AND allowed_purposes <@ ARRAY[
            'login', 'discover', 'article_detail', 'asset_download'
        ]::text[]
    ),
    -- 固定仅 login/discover；动态仅 article_detail/asset_download；本机可作默认回退
    CONSTRAINT egress_profiles_kind_purposes CHECK (
        (kind = 'local_direct'
            AND allowed_purposes <@ ARRAY[
                'login', 'discover', 'article_detail', 'asset_download'
            ]::text[])
        OR (kind = 'fixed'
            AND allowed_purposes <@ ARRAY['login', 'discover']::text[])
        OR (kind = 'dynamic'
            AND allowed_purposes <@ ARRAY[
                'article_detail', 'asset_download'
            ]::text[])
    ),
    -- 本机出口不挂供应商、不存端点密钥
    CONSTRAINT egress_profiles_local_shape CHECK (
        (kind = 'local_direct'
            AND provider_id IS NULL
            AND endpoint_secret_ref IS NULL)
        OR kind IN ('fixed', 'dynamic')
    )
);

CREATE INDEX IF NOT EXISTS idx_egress_profiles_kind_priority
    ON egress_profiles (kind, priority ASC, created_at ASC)
    WHERE status = 'active';

CREATE INDEX IF NOT EXISTS idx_egress_profiles_provider
    ON egress_profiles (provider_id)
    WHERE provider_id IS NOT NULL;

-- U04-07：默认本机出口种子（稳定 UUID，便于引用）
INSERT INTO egress_profiles (
    id,
    name,
    kind,
    provider_id,
    endpoint_secret_ref,
    priority,
    allowed_purposes,
    status
) VALUES (
    'a0000000-0000-4000-8000-000000000001',
    'local_direct',
    'local_direct',
    NULL,
    NULL,
    0,
    ARRAY['login', 'discover', 'article_detail', 'asset_download']::text[],
    'active'
)
ON CONFLICT (name) DO NOTHING;
