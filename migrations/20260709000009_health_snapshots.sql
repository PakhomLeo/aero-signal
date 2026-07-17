-- M11 daily-preflight：每日不可变健康快照（DATABASE_DESIGN §6）。

CREATE TABLE IF NOT EXISTS service_health_snapshots (
    id UUID PRIMARY KEY,
    snapshot_date DATE NOT NULL UNIQUE,
    generated_at TIMESTAMPTZ NOT NULL,
    overall_status TEXT NOT NULL
        CHECK (overall_status IN ('healthy', 'degraded', 'unhealthy')),
    yesterday_plan_success_count INT NOT NULL DEFAULT 0,
    yesterday_plan_failure_count INT NOT NULL DEFAULT 0,
    yesterday_article_failure_count INT NOT NULL DEFAULT 0,
    today_eligible_source_count INT NOT NULL DEFAULT 0,
    today_blocked_source_count INT NOT NULL DEFAULT 0,
    pending_action_count INT NOT NULL DEFAULT 0,
    summary_json JSONB NOT NULL DEFAULT '{}'::jsonb
);

CREATE TABLE IF NOT EXISTS service_health_components (
    id UUID PRIMARY KEY,
    snapshot_id UUID NOT NULL
        REFERENCES service_health_snapshots(id) ON DELETE CASCADE,
    component_type TEXT NOT NULL
        CHECK (component_type IN (
            'collector',
            'connector',
            'fixed_egress',
            'dynamic_egress',
            'worker',
            'object_store'
        )),
    component_id UUID,
    status TEXT NOT NULL
        CHECK (status IN ('healthy', 'degraded', 'unhealthy', 'not_configured')),
    reason_code TEXT,
    checked_at TIMESTAMPTZ NOT NULL,
    safe_details_json JSONB NOT NULL DEFAULT '{}'::jsonb
);

CREATE INDEX IF NOT EXISTS idx_service_health_components_snapshot
    ON service_health_components (snapshot_id);

-- 管理员「立即检查」独立运行记录（不覆盖当日快照）。
CREATE TABLE IF NOT EXISTS health_check_runs (
    id UUID PRIMARY KEY,
    job_id UUID REFERENCES fetch_jobs(id) ON DELETE SET NULL,
    target_type TEXT NOT NULL
        CHECK (target_type IN (
            'collector',
            'connector',
            'fixed_egress',
            'dynamic_egress',
            'worker',
            'object_store',
            'all'
        )),
    target_id UUID,
    status TEXT NOT NULL
        CHECK (status IN ('healthy', 'degraded', 'unhealthy', 'not_configured')),
    reason_code TEXT,
    safe_details_json JSONB NOT NULL DEFAULT '{}'::jsonb,
    checked_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_health_check_runs_checked
    ON health_check_runs (checked_at DESC);
