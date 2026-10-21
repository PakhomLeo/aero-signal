# Aero Signal

**English** | [简体中文](README.zh-CN.md)

<p align="center">
  <img src="docs/assets/architecture-overview.svg" alt="Aero Signal architecture overview" width="100%" />
</p>

Aero Signal is a self-hosted, single-server back office for **collecting, cleaning, versioning and incrementally delivering WeChat Official Account (公众号) articles** that its operator is authorised to process. It ties together monitored accounts, authorised collector logins, durable background jobs, article assets, OCR, daily health snapshots and a token-scoped delivery API in one auditable, recoverable workflow.

> **Authorised use only.** Aero Signal is not a scraping-evasion tool and not an Official Account search engine. It does **not** bypass CAPTCHAs or WAFs, does **not** spoof devices or rotate fingerprints, and does **not** perform unauthorised logins. See [Compliance and authorisation boundaries](#compliance-and-authorisation-boundaries).

## Table of contents

- [Highlights](#highlights)
- [Architecture](#architecture)
- [Admin console](#admin-console)
- [Tech stack](#tech-stack)
- [Repository layout](#repository-layout)
- [Quick start](#quick-start)
- [Configuration](#configuration)
- [API overview](#api-overview)
- [Testing](#testing)
- [Compliance and authorisation boundaries](#compliance-and-authorisation-boundaries)
- [Project status and roadmap](#project-status-and-roadmap)
- [Documentation](#documentation)
- [Contributing](#contributing)
- [License](#license)

## Highlights

- **Safe URL handling** – WeChat article URL canonicalisation, removal of sensitive query parameters (`uin`, `key`, `pass_ticket`, `appmsg_token`, …), strong article identity (`source + __biz + mid + idx`, canonical URL as fallback) and bounded, hop-by-hop validated short-link resolution.
- **Authorised collector accounts** – QR-code login state machine (`waiting → scanned → confirmed / expired / failed / cancelled`); credentials are kept in a SecretStore and only referenced from the database; each collector is bound to a fixed egress.
- **Discovery and backfill** – capability-based connectors (`mp_admin` for accounts the operator administers, a `fake` connector for development); bounded pagination, initial validation of at most three articles, and resumable, page-by-page history backfill.
- **HTTP-first capture** – bounded HTTP fetching with SSRF protection, explicit redirect checks, size limits and timeouts; an optional Chrome DevTools Protocol (CDP) browser renders lazy-loaded content only when needed.
- **Semantic content** – DOM allow-list cleaning to semantic HTML and Markdown that keeps headings, lists, quotes, code, tables, links, images and media placeholders.
- **Assets and OCR** – image classification, hash-deduplicated object storage (S3/MinIO) and OCR text anchored back into Markdown. A single failed asset never drops the article body; failed assets get their own retry job.
- **Durable jobs** – persistent jobs with idempotency keys, leases, retries, timeouts, cancellation and progress reporting (`crates/job-pipeline`).
- **Versioning and delivery** – article revision, change event and delivery outbox are committed atomically; consumers read incrementally by a monotonic `change_sequence`.
- **Daily operations** – 07:30 preflight, immutable daily health snapshot, 08:00 non-uniform daily plan and heartbeat-based promotion/demotion of monitored accounts.
- **Admin UI** – Vue 3 back office for the single administrator, plus a group-token API for downstream services.

## Architecture

Two binaries are built from the `signal-service` crate and share one composition root (`service/src/app.rs`):

| Binary | Role |
| --- | --- |
| `signal-service` | Axum HTTP server: `/api/admin/*` (admin), `/api/*` (delivery), `/healthz` |
| `signal-worker` | Polls and executes durable jobs; runs the daily scheduler (07:30 preflight / 08:00 plan) unless `SIGNAL_SCHEDULER=0` |

Dependencies flow `api → service → domain / repository → adapter / job`. API handlers only authenticate, validate and map errors; anything that takes longer than about two seconds is enqueued as a job. Every repository has an in-memory and a PostgreSQL implementation with the same behaviour.

### Collection pipeline and job flow

<p align="center">
  <img src="docs/assets/collection-pipeline.svg" alt="Collection pipeline and job flow" width="100%" />
</p>

Job kinds (from `AGENTS.md` / `crates/job-pipeline`): `daily_preflight`, `build_daily_plan`, `collector_probe`, `source_validation`, `source_validation_aggregate`, `fetch_source`, `history_backfill`, `fetch_article_detail`, `retry_article_assets`, `ocr_article_image`, `refetch_article`, `retention_cleanup`.

Job states: `queued`, `running`, `retry_wait`, `succeeded`, `failed`, `needs_action`, `cancelling`, `cancelled`, `dead_letter`.

### Data model and delivery

<p align="center">
  <img src="docs/assets/data-model-delivery.svg" alt="Data model and delivery flow" width="100%" />
</p>

Any change that affects what downstream consumers see (new article, refetch, manual edit, OCR or asset change) writes, in one transaction: the current article state, an immutable `article_revisions` row, an `article_change_events` row and a `delivery_outbox` row. See [`docs/DATABASE_DESIGN.md`](docs/DATABASE_DESIGN.md) for the full schema.

## Admin console

The Vue admin console (shown on the *Today's overview* page of a fresh local instance with no data yet):

<p align="center">
  <img src="docs/assets/admin-overview.png" alt="Admin console overview page" width="100%" />
</p>

## Tech stack

| Layer | Technology |
| --- | --- |
| Backend | Rust (edition 2024, `rust-version = 1.96`), Axum 0.8, Tokio, SQLx 0.8 |
| Data | PostgreSQL, Redis, S3-compatible object storage (MinIO) |
| Capture | Reqwest (rustls), `chromiumoxide` over CDP, pluggable connectors |
| Security | Argon2id admin passwords, hashed API tokens, SSRF guard, log redaction |
| Frontend | Vue 3, TypeScript, Vite 6, Vue Router, Pinia |
| Testing | In-memory repositories, fake connector / object store / OCR, HTML fixtures |

## Repository layout

```text
.
├─ service/                    # signal-service crate: HTTP API, services, domain, repositories, jobs, worker
│  ├─ src/api/admin/           # /api/admin routes (administrator session)
│  ├─ src/api/delivery/        # /api routes (delivery-group Bearer token)
│  ├─ src/service/             # use-case orchestration and policies
│  ├─ src/domain/              # state machines and business rules
│  ├─ src/repository/          # repository traits + memory + PostgreSQL implementations
│  ├─ src/jobs/                # job handlers and registry
│  ├─ src/adapters/            # SecretStore, QR ticket store (Redis), proxy provider
│  ├─ src/bin/signal_worker.rs # worker binary
│  └─ tests/                   # integration tests (fakes only)
├─ crates/
│  ├─ server-core/             # config, clock, errors, auth extractors, health, pagination, idempotency
│  ├─ job-pipeline/            # durable jobs: lease, retry, cancel, progress, safe context
│  ├─ platform-connectors/     # mp_admin / fake / weread (unsupported) capability adapters
│  ├─ crawler-capture/         # URL identity, HTTP/CDP capture, SSRF guard, extractor, assets, object store
│  └─ provider-hub/            # OCR provider trait + fake provider
├─ web/                        # Vue 3 admin UI
├─ migrations/                 # append-only SQLx migrations (applied automatically on start)
├─ scripts/                    # PowerShell helpers for local resources and fake smoke tests
├─ docs/                       # PRD, technical design, modules, database, API, deployment (Chinese)
├─ AGENTS.md                   # developer / agent onboarding guide (Chinese)
└─ SIGNAL_REPAIR_LOOP_PLAN.md  # record of the completed capture-pipeline repair work (Chinese)
```

## Quick start

The commands below are for Linux/macOS shells. The original project docs and `scripts/*.ps1` use PowerShell; equivalents are noted where relevant.

### 1. Prerequisites

- **Rust** via `rustup`. `rust-toolchain.toml` pins the `stable` channel (the workspace requires Rust ≥ 1.96) and also lists the `x86_64-pc-windows-msvc` target and `rustfmt`/`clippy`/`rust-analyzer` components, which `rustup` installs automatically on first use.
- **Node.js ≥ 22** and **pnpm 11** (`web/package.json` pins `pnpm@11.10.0`).
- For the full stack: **PostgreSQL**, **Redis** and an **S3-compatible object store** (MinIO or similar).
- Optional: a Chromium-based browser exposing a CDP endpoint, for lazy-loaded pages.

### 2. Fastest path: in-memory mode (no external services)

Memory mode uses in-memory repositories, an in-memory SecretStore and QR ticket store, and a fake object store (add `SIGNAL_OCR_PROVIDER=fake` for fake OCR). It is for development and testing only and is rejected when `SIGNAL_ENV=production`.

```bash
SIGNAL_STORE=memory SIGNAL_EMBED_WORKER=1 \
  cargo run -p signal-service --bin signal-service
```

> If a `.env` file exists, it is loaded automatically (via `dotenvy`). In memory mode, setting `REDIS_URL` or `S3_ENDPOINT` switches those adapters to the real Redis/S3 implementations.

The API listens on `http://127.0.0.1:8080`; `GET /healthz` returns `{"status":"ok","product":"signal"}`.

### 3. Full stack: PostgreSQL + Redis + S3 + separate worker

**Configure.**

```bash
cp .env.example .env          # PowerShell: Copy-Item .env.example .env
```

Then edit `.env`:

- set `SIGNAL_SECRET_STORE_PATH` (required in PostgreSQL mode, e.g. `./data/secrets`; `data/` is git-ignored);
- replace every local default credential before any non-local deployment.

**Initialise PostgreSQL** (role, password and database names must match `DATABASE_URL`):

```bash
sudo -u postgres psql -c "CREATE ROLE aero LOGIN PASSWORD 'change-me';"
sudo -u postgres psql -c "CREATE DATABASE aero_signal_dev OWNER aero;"
```

Migrations in `migrations/` are applied automatically when the service starts in PostgreSQL mode. You can also run them explicitly with the SQLx CLI: `sqlx migrate run`.

**Redis** needs no initialisation. All keys use `REDIS_KEY_PREFIX` (default `signal:`).

**Object storage:** create the bucket named by `S3_BUCKET` (default `aero-signal-dev`), for example with the MinIO client:

```bash
mc alias set local http://127.0.0.1:9000 "$S3_ACCESS_KEY" "$S3_SECRET_KEY"
mc mb --ignore-existing local/aero-signal-dev
```

On Windows with Podman, `scripts/init-dev-resources.ps1` creates the database and bucket inside existing containers named `aero-postgres` and `aero-minio`.

**Run the API and the worker** in two terminals:

```bash
cargo run -p signal-service --bin signal-service
cargo run -p signal-service --bin signal-worker
```

In PostgreSQL mode, `SIGNAL_EMBED_WORKER` is ignored, so the separate worker is required.

**Create the administrator** (once, before first login). The password must be 10–128 characters and contain letters and digits:

```bash
curl -X POST http://127.0.0.1:8080/api/admin/auth/bootstrap \
  -H 'content-type: application/json' \
  -d '{"username":"admin","password":"<a strong password>"}'
```

The first login enforces a password change. Do this on a private network: the bootstrap endpoint is unauthenticated until an administrator exists.

### 4. Frontend

```bash
cd web
pnpm install
pnpm dev        # http://127.0.0.1:5173, proxies /api to http://127.0.0.1:8080
```

Production build: `pnpm -C web build` (runs `vue-tsc -b && vite build`, output in `web/dist/`). Set `VITE_API_BASE_URL` to call the backend directly instead of the dev proxy.

## Configuration

All settings are environment variables (a `.env` file in the working directory is loaded on start). Source of truth: `crates/server-core/src/config.rs`, `service/src/adapters/factory.rs` and [`docs/部署说明.md`](docs/部署说明.md).

| Variable | Default | Description |
| --- | --- | --- |
| `SIGNAL_ENV` | `development` | `test` \| `development` \| `production`. Production checks every hard dependency before connecting and forbids fake fallbacks. |
| `SIGNAL_STORE` | auto | `memory` \| `postgres`. If unset, `postgres` when `DATABASE_URL` is set, otherwise `memory`. |
| `DATABASE_URL` | – | PostgreSQL connection string; required for `postgres`. |
| `SIGNAL_LISTEN_ADDR` | `127.0.0.1:8080` | HTTP listen address (`LISTEN_ADDR` is also accepted). |
| `SIGNAL_EMBED_WORKER` | `0` | Memory mode only: run the worker inside the HTTP process. |
| `SIGNAL_SCHEDULER` | `1` | Worker daily scheduler (07:30 preflight / 08:00 plan); `0` disables it. |
| `SIGNAL_WORKER_ID` | `signal-worker-<pid>` | Optional worker identifier. |
| `SIGNAL_SECRET_STORE_PATH` | – | Directory for the file SecretStore. Required in PostgreSQL and production modes. |
| `REDIS_URL` | – | Redis for QR login tickets. Required in PostgreSQL and production modes. |
| `REDIS_KEY_PREFIX` | `signal:` | Prefix for all Redis keys. |
| `S3_ENDPOINT` | – | S3/MinIO endpoint (path-style). Required in PostgreSQL and production modes; without it, memory mode uses a fake object store. |
| `S3_ACCESS_KEY` / `S3_SECRET_KEY` | – | Object storage credentials; required when `S3_ENDPOINT` is set. |
| `S3_BUCKET` | `aero-signal-dev` | Asset bucket. |
| `CLOAKBROWSER_CDP_URL` | – | CDP endpoint of a Chromium-compatible browser for lazy-load rendering, e.g. `http://127.0.0.1:9222`. Optional in development, required in production. |
| `SIGNAL_MP_ADMIN_PROTOCOL` | off | `1` / `true` / `yes` marks the real `mp_admin` QR-login and published-list protocol as ready; otherwise `mp_admin` reports itself as unsupported. The `fake` connector is registered in every non-production environment. |
| `SIGNAL_OCR_PROVIDER` | – | Only `fake` exists today; set `fake` to enable fake OCR outside production. Production refuses to start (`config.ocr_provider_not_implemented`) until a real provider is added. |
| `SIGNAL_PRODUCT` | `signal` | Product name reported by `/healthz`. |
| `SIGNAL_BOOTSTRAP_USERNAME` / `SIGNAL_BOOTSTRAP_PASSWORD` | – | Initial administrator credentials (see note below). Never commit real values. |
| `RUST_LOG` | – | Tracing filter, e.g. `info,signal=debug`. |

> **Note on bootstrap variables.** `AdministratorService::try_bootstrap_from_env` reads `SIGNAL_BOOTSTRAP_*`, but no binary calls it yet. Use `POST /api/admin/auth/bootstrap` as shown above.

Scheduling uses the timezone in `system_settings` (default `Asia/Shanghai`). Run the host or container in the same timezone, or set `TZ=Asia/Shanghai`.

## API overview

All error responses use `{"error":{"code":"module.reason_code","message":…,"details":…,"trace_id":…}}`. Error codes are listed in [`docs/api/错误码表.md`](docs/api/错误码表.md). Write requests accept an `Idempotency-Key` header.

### Admin API: `/api/admin` (administrator session, `Authorization: Bearer <session token>` from `/auth/login`)

| Area | Endpoints |
| --- | --- |
| Auth | `POST /auth/bootstrap`, `POST /auth/login`, `POST /auth/logout` |
| Account | `GET` / `PATCH /me` |
| Settings | `GET` / `PATCH /settings/schedule`; `GET` / `POST` / `PATCH /settings/egress-profiles`, `PATCH /settings/egress-profiles/{id}`, `POST /settings/egress-profiles/{id}/test`; `GET` / `POST` / `PATCH /settings/proxy-providers`, `PATCH /settings/proxy-providers/{id}` |
| Collector accounts | `GET` / `PATCH /collector-accounts`, `PATCH /collector-accounts/{id}`, `POST /collector-accounts/qr-sessions`, `GET` / `DELETE /collector-accounts/qr-sessions/{ticket}`, `POST /collector-accounts/{id}/probe-now`, `POST /collector-accounts/{id}/reauthorize`. Outside production only: `POST /collector-accounts/qr-sessions/{ticket}/advance` (fake QR flow). |
| Monitored sources | `POST /sources/validate`, `GET /source-validations/{id}`, `POST` / `GET /sources`, `GET` / `PATCH /sources/{id}`, `POST /sources/{id}/verify-again`, `POST /sources/{id}/fetch-now` |
| Articles | `GET /articles/{id}`, `PATCH /articles/{id}/content`, `POST /articles/{id}/reocr`, `POST /articles/{id}/refetch`, `POST /articles/{id}/revisions/{revision}/restore` |
| Health and plan | `GET /health-snapshots/latest`, `POST /health-snapshots/check-now`, `POST /health-snapshots/run-preflight`, `GET /daily-plan`, `POST /daily-plan/rebuild` |
| Jobs | `GET /jobs/{id}`, `POST /jobs/{id}/cancel` |
| Delivery groups | `GET` / `POST /delivery-groups`, `GET` / `PATCH /delivery-groups/{id}`, `GET` / `POST /delivery-groups/{id}/tokens`, `DELETE /delivery-groups/{id}/tokens/{token_id}` |

### Delivery API: `/api` (`Authorization: Bearer <group API token>`)

| Method | Path | Scope | Description |
| --- | --- | --- | --- |
| GET | `/articles?limit=` | `articles:read` | Articles of the token's group (default limit 50) |
| GET | `/articles/{id}` | `articles:read` | Article detail (current revision) |
| GET | `/articles/since?after=<change_sequence>&limit=` | `articles:read` | Incremental changes after a cursor (default limit 100) |
| GET | `/sources` | `sources:read` | Monitored accounts of the group |
| GET | `/service-health/latest` | `service_health:read` | Latest redacted daily health snapshot |
| GET | `/service-health?date=YYYY-MM-DD` | `service_health:read` | Immutable snapshot for a given day |

Plain-text tokens are shown only once, when they are created; only a hash is stored. Delivery responses never contain proxy endpoints, cookies, tokens, real collector IDs or raw error text. The contract is frozen by `service/tests/delivery.rs` (see [`docs/api/OpenAPI与契约冻结.md`](docs/api/OpenAPI与契约冻结.md)). No OpenAPI document is generated yet.

## Testing

All tests use fakes and fixtures. They never contact WeChat, proxies, real object storage or OCR services.

```bash
cargo build --workspace
cargo test --workspace
cargo fmt --all -- --check
cargo clippy -p platform-connectors -p crawler-capture -p signal-service --tests -- -D warnings
pnpm -C web build
```

> As of this writing, the Clippy command is clean on Rust 1.96.1. Newer stable toolchains (1.99.1) report `clippy::double_must_use` at `#[async_trait]` in `crates/provider-hub/src/ocr.rs`. To reproduce the reference result, run `cargo +1.96 clippy …`.

Targeted suites for day-to-day work:

```bash
cargo test -p platform-connectors protocol    # connector protocol and pagination
cargo test -p crawler-capture                 # URL identity, extraction, SSRF, assets
cargo test -p job-pipeline                    # lease / retry / cancel
cargo test -p signal-service --lib            # service logic
cargo test -p signal-service --test source            # source validation
cargo test -p signal-service --test article_capture   # capture, asset retry, backfill
```

## Compliance and authorisation boundaries

These rules are part of the product's design, not optional settings. Contributions that weaken them will not be accepted.

- **No protection bypass.** CAPTCHAs, login walls, WAF challenges and permission errors are classified as `needs_action` for a human to handle. There is no CAPTCHA solving, WAF bypass, mouse-movement simulation, MITM or similar evasion.
- **No device spoofing.** No device-fingerprint rotation or device impersonation. Platform limits never trigger automatic egress rotation to keep requesting.
- **No unauthorised login.** Collector accounts exist only after the administrator completes a QR login. `mp_admin` works only for accounts the operator is authorised to manage and cannot enumerate arbitrary third-party accounts. `weread` stays explicitly unsupported until it has been authorised and accepted.
- **Credential hygiene.** Cookies, tokens, QR tickets, storage state and proxy passwords go only to the SecretStore or short-lived Redis entries. They never go to ordinary database columns, logs, audit details or job safe context.
- **Network safety.** Every fetched URL goes through the SSRF guard, hop-by-hop redirect validation, response size limits and timeouts.
- **Your responsibility.** You are responsible for complying with the WeChat platform's terms, copyright and the laws that apply to you. Only collect and redistribute content you are entitled to process.

## Project status and roadmap

**Status (per `AGENTS.md`, 2026-07-17):** the capture-pipeline repair plan is complete and was verified locally. That work covered candidate metadata, old-article detection, short-link safety, incremental pagination, history backfill, semantic body extraction, partial asset success, structured errors and multi-dimensional rate limiting.

**Known limitations and next steps** (from `AGENTS.md` §13 and `docs/`):

- [ ] Integrate and accept a real OCR provider. Only the fake provider exists, so `SIGNAL_ENV=production` refuses to start.
- [ ] Keep `weread` unsupported until authorisation, protocol freeze and acceptance.
- [ ] Deployment-level setup and acceptance for the production SecretStore, proxy provider, object storage and browser runtime.
- [ ] Optional machine-readable OpenAPI for the delivery API (`docs/api/openapi-delivery.yaml`).
- [ ] Wire up environment-based administrator bootstrap (`SIGNAL_BOOTSTRAP_*`).
- [ ] Add regression tests with redacted fixtures as platform DOM/protocols change.

The system is deliberately single-tenant with a single administrator. Multi-user auditing would be a separate product upgrade, not hidden roles.

## Documentation

The detailed docs are in Chinese:

- [AGENTS.md](AGENTS.md): developer onboarding, module map, rules and workflow
- [docs/PRD.md](docs/PRD.md): product requirements
- [docs/TECHNICAL_SOLUTION.md](docs/TECHNICAL_SOLUTION.md): technical design
- [docs/BACKEND_MODULES.md](docs/BACKEND_MODULES.md): backend modules
- [docs/DATABASE_DESIGN.md](docs/DATABASE_DESIGN.md): database design
- [docs/api/OpenAPI与契约冻结.md](docs/api/OpenAPI与契约冻结.md): API contract
- [docs/api/错误码表.md](docs/api/错误码表.md): error codes
- [docs/部署说明.md](docs/部署说明.md): deployment

## Contributing

Issues and pull requests are welcome. Please read [CONTRIBUTING.md](CONTRIBUTING.md) first. In short: keep the compliance boundaries, add migrations instead of editing applied ones, keep memory and PostgreSQL repositories consistent, test with fakes and fixtures only, and never commit credentials or real platform responses.

## License

Released under the [MIT License](LICENSE).

> `Cargo.toml` currently declares `license = "MIT OR Apache-2.0"` in its workspace metadata. If dual licensing is intended, add the Apache-2.0 text and keep the metadata consistent.
