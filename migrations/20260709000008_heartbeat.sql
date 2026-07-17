-- M08 heartbeat：source_heartbeat_events + 规则表辅助字段。

CREATE TABLE IF NOT EXISTS source_heartbeat_events (
    id UUID PRIMARY KEY,
    source_account_id UUID NOT NULL
        REFERENCES source_accounts(id) ON DELETE CASCADE,
    outcome TEXT NOT NULL
        CHECK (outcome IN (
            'new_article',
            'empty_success',
            'technical_failure',
            'access_blocked',
            'manual'
        )),
    level_before TEXT NOT NULL,
    level_after TEXT NOT NULL,
    empty_count_after INT NOT NULL,
    previous_next_at TIMESTAMPTZ,
    next_heartbeat_at TIMESTAMPTZ,
    reason_code TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_source_heartbeat_events_source
    ON source_heartbeat_events (source_account_id, created_at DESC);

-- 技术失败连续计数（不计空抓）；L3 低活跃标记。
DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM information_schema.columns
        WHERE table_name = 'account_fetch_rules'
          AND column_name = 'consecutive_technical_failures'
    ) THEN
        ALTER TABLE account_fetch_rules
            ADD COLUMN consecutive_technical_failures INT NOT NULL DEFAULT 0;
    END IF;
    IF NOT EXISTS (
        SELECT 1 FROM information_schema.columns
        WHERE table_name = 'account_fetch_rules'
          AND column_name = 'low_activity'
    ) THEN
        ALTER TABLE account_fetch_rules
            ADD COLUMN low_activity BOOLEAN NOT NULL DEFAULT false;
    END IF;
END $$;
