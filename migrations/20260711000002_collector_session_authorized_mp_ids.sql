-- R02: persist the public-account scope granted to a collector login session.
ALTER TABLE collector_login_sessions
    ADD COLUMN IF NOT EXISTS authorized_mp_ids JSONB NOT NULL DEFAULT '[]'::jsonb;

ALTER TABLE collector_login_sessions
    ADD CONSTRAINT collector_login_sessions_authorized_mp_ids_array
    CHECK (jsonb_typeof(authorized_mp_ids) = 'array');
