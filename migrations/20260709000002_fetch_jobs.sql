-- M02 job-pipeline：统一长任务表（DATABASE_DESIGN 中的 fetch_jobs）。
-- safe_context_json 仅允许脱敏字段，由应用层白名单校验。

CREATE EXTENSION IF NOT EXISTS pgcrypto;

CREATE TABLE IF NOT EXISTS fetch_jobs (
    id UUID PRIMARY KEY,
    kind TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'queued'
        CHECK (status IN (
            'queued', 'running', 'retry_wait', 'succeeded', 'failed',
            'needs_action', 'cancelling', 'cancelled', 'dead_letter'
        )),
    payload_json JSONB NOT NULL DEFAULT '{}'::jsonb,
    idempotency_key TEXT,
    progress_percent DOUBLE PRECISION NOT NULL DEFAULT 0,
    progress_message TEXT NOT NULL DEFAULT '',
    progress_stage TEXT NOT NULL DEFAULT 'created',
    attempt_count INTEGER NOT NULL DEFAULT 0,
    max_attempts INTEGER NOT NULL DEFAULT 3,
    next_run_at TIMESTAMPTZ,
    locked_by TEXT,
    locked_until TIMESTAMPTZ,
    lease_heartbeat_at TIMESTAMPTZ,
    execution_timeout_at TIMESTAMPTZ,
    priority INTEGER NOT NULL DEFAULT 0,
    dependency_keys JSONB NOT NULL DEFAULT '[]'::jsonb,
    cost_budget_limit DOUBLE PRECISION,
    cancel_requested_at TIMESTAMPTZ,
    cancelled_at TIMESTAMPTZ,
    resume_token TEXT,
    result_json JSONB,
    last_error_code TEXT,
    last_error_message TEXT,
    action_code TEXT,
    action_message TEXT,
    action_url_or_command TEXT,
    safe_context_json JSONB,
    created_by TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- 幂等键唯一（允许多个 NULL）。
CREATE UNIQUE INDEX IF NOT EXISTS uq_fetch_jobs_idempotency_key
    ON fetch_jobs (idempotency_key)
    WHERE idempotency_key IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_fetch_jobs_due
    ON fetch_jobs (status, priority DESC, next_run_at, created_at);

CREATE INDEX IF NOT EXISTS idx_fetch_jobs_kind_status
    ON fetch_jobs (kind, status);

CREATE INDEX IF NOT EXISTS idx_fetch_jobs_lease
    ON fetch_jobs (status, locked_until)
    WHERE status = 'running';

-- 可选：attempt 明细表，供后续审计/诊断。
CREATE TABLE IF NOT EXISTS fetch_attempts (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    job_id UUID NOT NULL REFERENCES fetch_jobs(id) ON DELETE CASCADE,
    attempt_no INTEGER NOT NULL,
    worker_id TEXT,
    started_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    finished_at TIMESTAMPTZ,
    status TEXT NOT NULL DEFAULT 'running',
    error_code TEXT,
    error_message TEXT,
    safe_context_json JSONB,
    UNIQUE (job_id, attempt_no)
);

CREATE INDEX IF NOT EXISTS idx_fetch_attempts_job
    ON fetch_attempts (job_id, attempt_no);
