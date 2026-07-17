-- A collector may have one active login session, while retaining any number
-- of revoked historical sessions. PostgreSQL NULLs do not enforce that with a
-- normal composite UNIQUE constraint, so replace it with a partial index.
ALTER TABLE collector_login_sessions
    DROP CONSTRAINT IF EXISTS collector_login_sessions_collector_account_id_revoked_at_key;

CREATE UNIQUE INDEX IF NOT EXISTS uq_collector_login_sessions_active
    ON collector_login_sessions (collector_account_id)
    WHERE revoked_at IS NULL;
