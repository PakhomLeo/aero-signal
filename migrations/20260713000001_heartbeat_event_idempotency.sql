-- Heartbeat application is keyed by the fetch command that produced it.
-- Existing rows predate the key; their immutable id is a safe one-time key.
ALTER TABLE source_heartbeat_events
    ADD COLUMN IF NOT EXISTS event_key TEXT;

UPDATE source_heartbeat_events
SET event_key = id::text
WHERE event_key IS NULL;

ALTER TABLE source_heartbeat_events
    ALTER COLUMN event_key SET NOT NULL;

CREATE UNIQUE INDEX IF NOT EXISTS uq_source_heartbeat_events_event_key
    ON source_heartbeat_events (source_account_id, event_key);
