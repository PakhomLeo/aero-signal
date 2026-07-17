-- M09 article-capture：文章链接、初表、资产与图片决策。
-- source_accounts 为最小桩（M07 将扩展列与关联表）；本迁移仅保证 FK 可用。

CREATE TABLE IF NOT EXISTS source_accounts (
    id UUID PRIMARY KEY,
    display_name TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'active'
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

CREATE TABLE IF NOT EXISTS article_links (
    id UUID PRIMARY KEY,
    source_account_id UUID NOT NULL
        REFERENCES source_accounts(id) ON DELETE CASCADE,
    canonical_url TEXT NOT NULL,
    wechat_biz TEXT,
    wechat_mid TEXT,
    wechat_idx TEXT,
    wechat_sn TEXT,
    title_hint TEXT,
    discovered_via TEXT NOT NULL,
    discovered_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (source_account_id, wechat_biz, wechat_mid, wechat_idx),
    UNIQUE (source_account_id, canonical_url)
);

CREATE INDEX IF NOT EXISTS idx_article_links_source_discovered
    ON article_links (source_account_id, discovered_at DESC);

CREATE TABLE IF NOT EXISTS articles (
    id UUID PRIMARY KEY,
    source_account_id UUID NOT NULL
        REFERENCES source_accounts(id) ON DELETE RESTRICT,
    article_link_id UUID
        REFERENCES article_links(id) ON DELETE SET NULL,
    original_url TEXT NOT NULL,
    canonical_url TEXT NOT NULL,
    original_title TEXT NOT NULL,
    author TEXT,
    published_at TIMESTAMPTZ,
    fetched_at TIMESTAMPTZ NOT NULL,
    current_revision INT NOT NULL DEFAULT 1,
    content_md TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    asset_manifest_hash TEXT,
    status TEXT NOT NULL DEFAULT 'captured'
        CHECK (status IN (
            'captured',
            'ready',
            'ready_with_ocr_warning',
            'failed',
            'withdrawn'
        )),
    withdrawn_at TIMESTAMPTZ,
    UNIQUE (source_account_id, canonical_url)
);

CREATE INDEX IF NOT EXISTS idx_articles_source_fetched
    ON articles (source_account_id, fetched_at DESC);

CREATE TABLE IF NOT EXISTS article_assets (
    id UUID PRIMARY KEY,
    article_id UUID NOT NULL
        REFERENCES articles(id) ON DELETE CASCADE,
    original_url TEXT,
    storage_key TEXT NOT NULL,
    asset_hash TEXT,
    width INT,
    height INT,
    byte_size BIGINT,
    mime_type TEXT,
    asset_kind TEXT NOT NULL
        CHECK (asset_kind IN ('body_image', 'raw_html', 'screenshot')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_article_assets_hash_dedup
    ON article_assets (article_id, asset_hash)
    WHERE asset_hash IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_article_assets_article
    ON article_assets (article_id);

CREATE TABLE IF NOT EXISTS article_image_decisions (
    id UUID PRIMARY KEY,
    article_id UUID NOT NULL
        REFERENCES articles(id) ON DELETE CASCADE,
    candidate_url TEXT NOT NULL,
    decision TEXT NOT NULL
        CHECK (decision IN (
            'kept',
            'decorative',
            'too_small',
            'hidden',
            'duplicate',
            'unsupported'
        )),
    measured_width INT,
    measured_height INT,
    reason_json JSONB NOT NULL DEFAULT '{}'::jsonb,
    asset_id UUID
        REFERENCES article_assets(id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_article_image_decisions_article
    ON article_image_decisions (article_id);
