# Idempotent: create Signal local DB aero_signal_dev and MinIO bucket aero-signal-dev.
# Usage:
#   .\scripts\init-dev-resources.ps1

$ErrorActionPreference = "Stop"

$DbName = "aero_signal_dev"
$Bucket = "aero-signal-dev"
$PgContainer = "aero-postgres"
$MinioContainer = "aero-minio"
$MinioAlias = "aero-local"
$MinioEndpoint = "http://127.0.0.1:9000"
$MinioAccess = "aero"
$MinioSecret = "aero_local_dev"
$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$PyBucketScript = Join-Path $ScriptDir "create-minio-bucket.py"

function Assert-Command([string]$Name) {
    if (-not (Get-Command $Name -ErrorAction SilentlyContinue)) {
        throw "Command not found: $Name"
    }
}

Assert-Command "podman"

Write-Host "==> Checking containers..."
$pgRunning = podman ps --format "{{.Names}}" | Where-Object { $_ -eq $PgContainer }
if (-not $pgRunning) {
    throw "Container $PgContainer is not running. Start your local PostgreSQL/Redis/MinIO stack first."
}

Write-Host "==> Ensuring database $DbName exists..."
$exists = podman exec $PgContainer psql -U aero -d aero_base -tAc "SELECT 1 FROM pg_database WHERE datname='$DbName';"
if ($exists -match "1") {
    Write-Host "    Database already exists, skip."
} else {
    podman exec $PgContainer psql -U aero -d aero_base -c "CREATE DATABASE $DbName;"
    Write-Host "    Created database $DbName."
}

Write-Host "==> Ensuring MinIO bucket $Bucket exists..."
$minioRunning = podman ps --format "{{.Names}}" | Where-Object { $_ -eq $MinioContainer }
if (-not $minioRunning) {
    Write-Warning "Container $MinioContainer is not running; skip bucket creation."
    Write-Host "    Open http://127.0.0.1:9001 , login aero / aero_local_dev , create bucket $Bucket."
} else {
    $mcAvailable = Get-Command mc -ErrorAction SilentlyContinue
    $created = $false

    if ($mcAvailable) {
        & mc alias set $MinioAlias $MinioEndpoint $MinioAccess $MinioSecret | Out-Null
        $bucketList = & mc ls "$MinioAlias/" 2>$null | Out-String
        if ($bucketList -match [regex]::Escape($Bucket)) {
            Write-Host "    Bucket already exists, skip."
            $created = $true
        } else {
            & mc mb "$MinioAlias/$Bucket"
            Write-Host "    Created bucket $Bucket."
            $created = $true
        }
    }

    if (-not $created) {
        try {
            podman exec $MinioContainer sh -c "command -v mc >/dev/null 2>&1"
            if ($LASTEXITCODE -eq 0) {
                podman exec $MinioContainer mc alias set local http://127.0.0.1:9000 $MinioAccess $MinioSecret | Out-Null
                podman exec $MinioContainer mc mb --ignore-existing "local/$Bucket" | Out-Null
                Write-Host "    Ensured bucket $Bucket via container mc."
                $created = $true
            }
        } catch {
            $created = $false
        }
    }

    if (-not $created) {
        $aws = Get-Command aws -ErrorAction SilentlyContinue
        if ($aws) {
            $env:AWS_ACCESS_KEY_ID = $MinioAccess
            $env:AWS_SECRET_ACCESS_KEY = $MinioSecret
            aws --endpoint-url $MinioEndpoint s3 mb "s3://$Bucket" 2>$null
            # exit 0 created; exit 1 often means already exists
            if ($LASTEXITCODE -eq 0 -or $LASTEXITCODE -eq 1) {
                Write-Host "    Ensured bucket $Bucket via aws cli."
                $created = $true
            }
        }
    }

    # Fallback: Python stdlib SigV4 CreateBucket (no boto3)
    if (-not $created) {
        $py = Get-Command python -ErrorAction SilentlyContinue
        if ($py -and (Test-Path $PyBucketScript)) {
            $pyOut = & python $PyBucketScript `
                --endpoint $MinioEndpoint `
                --access-key $MinioAccess `
                --secret-key $MinioSecret `
                --bucket $Bucket 2>&1
            if ($LASTEXITCODE -eq 0) {
                Write-Host "    Ensured bucket $Bucket via Python SigV4 ($pyOut)."
                $created = $true
            } else {
                Write-Warning "Python SigV4 CreateBucket failed: $pyOut"
            }
        }
    }

    if (-not $created) {
        Write-Warning "Could not auto-create MinIO bucket (mc/aws/Python unavailable)."
        Write-Host "    Open http://127.0.0.1:9001"
        Write-Host "    Login: aero / aero_local_dev"
        Write-Host "    Create bucket: $Bucket"
        Write-Host "    Or install MinIO Client (mc) and re-run this script."
    }
}

Write-Host "==> Done. DATABASE_URL example:"
Write-Host "    postgres://aero:aero_local_dev@127.0.0.1:5432/$DbName"
