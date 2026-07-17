# Signal 演示用 Fake 冒烟：不连真实微信/代理/OCR。
# 用法：在仓库根目录执行 .\scripts\demo-fake-smoke.ps1

$ErrorActionPreference = "Stop"
Set-Location $PSScriptRoot\..

if (-not $env:DATABASE_URL) {
    $env:DATABASE_URL = "postgres://aero:aero_local_dev@127.0.0.1:5432/aero_signal_dev"
}

Write-Host "== sqlx migrate run =="
sqlx migrate run

Write-Host "== cargo test retention / redact / audit =="
cargo test -p signal-service -- retention
cargo test -p signal-service -- redact
cargo test -p signal-service -- audit

Write-Host "Fake smoke OK（未调用真实平台）"
