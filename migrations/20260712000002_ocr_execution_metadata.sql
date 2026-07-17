ALTER TABLE article_ocr_blocks
    ADD COLUMN IF NOT EXISTS provider TEXT,
    ADD COLUMN IF NOT EXISTS model TEXT,
    ADD COLUMN IF NOT EXISTS provider_version TEXT,
    ADD COLUMN IF NOT EXISTS ocr_version TEXT,
    ADD COLUMN IF NOT EXISTS provider_call_id TEXT,
    ADD COLUMN IF NOT EXISTS error_code TEXT,
    ADD COLUMN IF NOT EXISTS attempt INT NOT NULL DEFAULT 0;

CREATE INDEX IF NOT EXISTS idx_article_ocr_blocks_provider_call
    ON article_ocr_blocks (provider_call_id)
    WHERE provider_call_id IS NOT NULL;
