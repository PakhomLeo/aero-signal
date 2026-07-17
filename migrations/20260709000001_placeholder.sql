-- Signal 脚手架占位迁移：建立 _signal_scaffold 标记表，便于验证 sqlx migrate。
-- 后续业务表按 docs/DATABASE_DESIGN.md 通过新迁移追加。

CREATE TABLE IF NOT EXISTS _signal_scaffold (
    id SMALLINT PRIMARY KEY DEFAULT 1 CHECK (id = 1),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

INSERT INTO _signal_scaffold (id)
VALUES (1)
ON CONFLICT (id) DO NOTHING;
