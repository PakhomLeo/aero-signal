-- 首期无独立 failed_validation_asset 表：禁用该保留策略，避免清理任务空转误导。
-- 幂等：重复执行仍为 enabled=false。

UPDATE retention_policies
SET enabled = FALSE,
    updated_at = now()
WHERE target_kind = 'failed_validation_asset'
  AND enabled = TRUE;
