ALTER TABLE source_accounts
    ADD COLUMN IF NOT EXISTS version BIGINT NOT NULL DEFAULT 1;

ALTER TABLE account_fetch_rules
    ADD COLUMN IF NOT EXISTS auto_promotion_enabled BOOLEAN NOT NULL DEFAULT true;

ALTER TABLE source_accounts
    DROP CONSTRAINT IF EXISTS source_accounts_version_positive;
ALTER TABLE source_accounts
    ADD CONSTRAINT source_accounts_version_positive CHECK (version >= 1);

ALTER TABLE account_fetch_rules
    DROP CONSTRAINT IF EXISTS account_fetch_rules_interval_range;
ALTER TABLE account_fetch_rules
    ADD CONSTRAINT account_fetch_rules_interval_range
    CHECK (interval_override_hours IS NULL OR interval_override_hours BETWEEN 1 AND 8760);
