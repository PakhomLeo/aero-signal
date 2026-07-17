-- M15：数据保留策略。清理任务删除前必须做业务引用检查。

CREATE TABLE IF NOT EXISTS retention_policies (
    id UUID PRIMARY KEY,
    target_kind TEXT NOT NULL UNIQUE
        CHECK (target_kind IN (
            'raw_html',
            'screenshot',
            'body_image',
            'ocr_text',
            'api_call_log',
            'audit_log',
            'change_event',
            'delivery_outbox',
            'failed_validation_asset'
        )),
    retention_days INT NOT NULL CHECK (retention_days > 0),
    hard_delete_after_days INT
        CHECK (
            hard_delete_after_days IS NULL
            OR hard_delete_after_days >= retention_days
        ),
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- 默认策略（可后续由管理员调整；此处仅种子，不写密钥）。
INSERT INTO retention_policies (id, target_kind, retention_days, hard_delete_after_days, enabled)
VALUES
    ('a0000000-0000-4000-8000-000000000001', 'raw_html', 30, 90, TRUE),
    ('a0000000-0000-4000-8000-000000000002', 'screenshot', 30, 90, TRUE),
    ('a0000000-0000-4000-8000-000000000003', 'body_image', 180, NULL, TRUE),
    ('a0000000-0000-4000-8000-000000000004', 'ocr_text', 180, NULL, TRUE),
    ('a0000000-0000-4000-8000-000000000005', 'api_call_log', 90, 180, TRUE),
    ('a0000000-0000-4000-8000-000000000006', 'audit_log', 365, NULL, TRUE),
    ('a0000000-0000-4000-8000-000000000007', 'change_event', 365, NULL, TRUE),
    ('a0000000-0000-4000-8000-000000000008', 'delivery_outbox', 30, 90, TRUE),
    ('a0000000-0000-4000-8000-000000000009', 'failed_validation_asset', 14, 30, TRUE)
ON CONFLICT (target_kind) DO NOTHING;
