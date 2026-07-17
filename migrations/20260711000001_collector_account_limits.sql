-- R02: persist collector cooldown and the remaining daily request budget.
-- NULL means that no cooldown/budget limit is currently configured.
ALTER TABLE collector_accounts
    ADD COLUMN IF NOT EXISTS cooldown_until TIMESTAMPTZ,
    ADD COLUMN IF NOT EXISTS daily_budget_remaining BIGINT
        CHECK (daily_budget_remaining IS NULL OR daily_budget_remaining >= 0);

CREATE INDEX IF NOT EXISTS idx_collector_accounts_cooldown
    ON collector_accounts (cooldown_until)
    WHERE cooldown_until IS NOT NULL;
