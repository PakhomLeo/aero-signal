# SQLx 迁移

迁移文件位于本目录，由 `sqlx migrate` 管理。

```powershell
Copy-Item .env.example .env
$env:DATABASE_URL = "postgres://aero:aero_local_dev@127.0.0.1:5432/aero_signal_dev"
sqlx migrate info
sqlx migrate run
```

业务表按 `docs/DATABASE_DESIGN.md` 追加；已经应用的迁移不得改写。
