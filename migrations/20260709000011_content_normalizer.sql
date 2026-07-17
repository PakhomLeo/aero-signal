-- M10 content-normalizer：revision / OCR / change_events / delivery_outbox。
-- 兼容已有 articles 表（M09）；不改 articles 列定义。

CREATE TABLE IF NOT EXISTS article_revisions (
    id UUID PRIMARY KEY,
    article_id UUID NOT NULL
        REFERENCES articles(id) ON DELETE CASCADE,
    revision INT NOT NULL,
    editor_kind TEXT NOT NULL
        CHECK (editor_kind IN ('crawler', 'ocr', 'administrator')),
    content_md TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    change_reason TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (article_id, revision)
);

CREATE INDEX IF NOT EXISTS idx_article_revisions_article
    ON article_revisions (article_id, revision DESC);

CREATE TABLE IF NOT EXISTS article_ocr_blocks (
    id UUID PRIMARY KEY,
    article_id UUID NOT NULL
        REFERENCES articles(id) ON DELETE CASCADE,
    asset_id UUID NOT NULL
        REFERENCES article_assets(id) ON DELETE CASCADE,
    block_anchor TEXT NOT NULL,
    recognized_text TEXT,
    confidence NUMERIC(5, 4),
    status TEXT NOT NULL
        CHECK (status IN ('pending', 'succeeded', 'failed', 'manual_override')),
    provider_ref TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (article_id, asset_id, block_anchor)
);

CREATE INDEX IF NOT EXISTS idx_article_ocr_blocks_article
    ON article_ocr_blocks (article_id);

CREATE TABLE IF NOT EXISTS article_change_events (
    id UUID PRIMARY KEY,
    event_id UUID NOT NULL UNIQUE,
    change_sequence BIGSERIAL NOT NULL UNIQUE,
    article_id UUID NOT NULL
        REFERENCES articles(id) ON DELETE RESTRICT,
    article_revision INT NOT NULL,
    change_type TEXT NOT NULL
        CHECK (change_type IN (
            'article_created',
            'article_updated',
            'article_refetched',
            'article_edited',
            'article_assets_changed',
            'article_withdrawn'
        )),
    schema_version INT NOT NULL,
    payload_json JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_article_change_events_sequence
    ON article_change_events (change_sequence);

CREATE INDEX IF NOT EXISTS idx_article_change_events_article
    ON article_change_events (article_id, change_sequence);

CREATE TABLE IF NOT EXISTS delivery_outbox (
    id UUID PRIMARY KEY,
    event_id UUID NOT NULL UNIQUE,
    change_sequence BIGINT NOT NULL,
    event_type TEXT NOT NULL
        CHECK (event_type IN (
            'article_created',
            'article_updated',
            'article_refetched',
            'article_edited',
            'article_assets_changed',
            'article_withdrawn'
        )),
    article_id UUID NOT NULL
        REFERENCES articles(id) ON DELETE RESTRICT,
    article_revision INT NOT NULL,
    payload_schema_version INT NOT NULL,
    payload_json JSONB NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending'
        CHECK (status IN (
            'pending',
            'delivering',
            'delivered',
            'failed',
            'dead_letter'
        )),
    attempt_count INT NOT NULL DEFAULT 0,
    next_attempt_at TIMESTAMPTZ,
    last_error_code TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    delivered_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_delivery_outbox_pending
    ON delivery_outbox (status, next_attempt_at, change_sequence);

CREATE INDEX IF NOT EXISTS idx_delivery_outbox_sequence
    ON delivery_outbox (change_sequence);
