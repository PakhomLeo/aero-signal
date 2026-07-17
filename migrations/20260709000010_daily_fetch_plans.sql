-- M12 daily-plan：每日非均匀抓取计划（DATABASE_DESIGN §6）。

CREATE TABLE IF NOT EXISTS daily_fetch_plans (
    id UUID PRIMARY KEY,
    plan_date DATE NOT NULL,
    source_account_id UUID NOT NULL
        REFERENCES source_accounts(id) ON DELETE RESTRICT,
    health_snapshot_id UUID NOT NULL
        REFERENCES service_health_snapshots(id) ON DELETE RESTRICT,
    planned_run_at TIMESTAMPTZ NOT NULL,
    status TEXT NOT NULL DEFAULT 'planned'
        CHECK (status IN (
            'planned',
            'running',
            'succeeded',
            'retry_wait',
            'failed',
            'needs_action',
            'skipped'
        )),
    reason_code TEXT NOT NULL,
    heartbeat_event_id UUID
        REFERENCES source_heartbeat_events(id) ON DELETE SET NULL,
    fetch_job_id UUID
        REFERENCES fetch_jobs(id) ON DELETE SET NULL,
    collector_account_id UUID
        REFERENCES collector_accounts(id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (plan_date, source_account_id)
);

CREATE INDEX IF NOT EXISTS idx_daily_plan_ready
    ON daily_fetch_plans (status, planned_run_at);

CREATE INDEX IF NOT EXISTS idx_daily_plan_date
    ON daily_fetch_plans (plan_date);
