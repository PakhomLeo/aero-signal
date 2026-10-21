# Contributing to Aero Signal

Thank you for helping improve Aero Signal. This project keeps a pragmatic review bar: changes should be small, easy to run locally, covered by focused tests, and consistent with the existing architecture and compliance boundaries.

Before you start, read [`AGENTS.md`](AGENTS.md) (developer onboarding, in Chinese) and the documents it points to for the area you are changing.

## Ground rules (non-negotiable)

Pull requests that break any of these rules will not be merged:

- **No protection bypass or evasion.** No CAPTCHA solving, WAF bypass, mouse simulation, device-fingerprint rotation, MITM, unauthorised login, or automatic egress rotation after a platform limit. Platform protection is reported as `needs_action`.
- **Authorised access only.** `mp_admin` works only for accounts the operator is authorised to manage. Do not add enumeration of arbitrary third-party accounts. `weread` stays explicitly unsupported until it is authorised and accepted.
- **Single administrator.** Do not add registration, ordinary users, roles, membership or billing.
- **Keep secrets out.** Cookies, tokens, QR tickets, storage state and proxy passwords belong in the SecretStore or short-lived Redis entries only. Never put them in database JSON, logs, audit details, job safe context, fixtures or commits.
- **Keep network safety.** Every URL fetch keeps the SSRF guard, hop-by-hop redirect checks, response size limits and timeouts.
- **Tests never touch real services.** Unit and integration tests use fakes and fixtures. They never call WeChat, proxies, real S3 or OCR providers.

## Development setup

1. Install Rust with `rustup` (the toolchain comes from `rust-toolchain.toml`; Rust ≥ 1.96), Node.js ≥ 22 and pnpm 11.
2. For the full stack, run PostgreSQL, Redis and an S3-compatible object store. In-memory mode needs none of them (see the [README](README.md#quick-start)).
3. `cp .env.example .env` and adjust local values. Never commit `.env`.
4. Frontend: `cd web && pnpm install`.

## Quality checks

Run the checks that cover your change before opening a pull request:

```bash
cargo fmt --all -- --check
cargo test -p <affected crate>                       # e.g. crawler-capture, job-pipeline
cargo test -p signal-service --lib
cargo test -p signal-service --tests
cargo clippy -p platform-connectors -p crawler-capture -p signal-service --tests -- -D warnings
pnpm -C web build                                    # when web/ or API DTOs change
```

## Code guidelines

- Follow the dependency direction `api → service → domain / repository → adapter / job`. API handlers only authenticate, validate and map errors. Anything that takes longer than about two seconds is a job.
- Put state transitions in domain methods. Do not jump between states directly in API or SQL code.
- Keep the in-memory and PostgreSQL repositories behaviourally identical.
- **Migrations are append-only.** Add a new file under `migrations/`; never edit one that has already been applied.
- A new job kind needs a kind constant, an idempotency key, a payload, a handler, registry wiring, repository behaviour and targeted tests.
- Inject external capabilities through traits, with fake implementations, timeouts, cancellation, rate limiting and redacted errors.
- Use the injected `Clock` for time logic; tests must not rely on real sleeps.
- Changes visible downstream must write article state, revision, change event and outbox in one transaction.
- When backend DTOs change, update `web/src/api/types.ts` and the error labels.
- Add regression fixtures for platform changes only as **redacted** fixtures, never real responses.

## Pull requests

- Keep each pull request to one behaviour or one cleanup theme.
- Describe what changed, why, and which tests you ran.
- Update the README, `AGENTS.md` or `docs/` when you change documented behaviour, configuration or APIs.
- Document new environment variables in `.env.example` (placeholders only) and in the README configuration table.

## Reporting security issues

Please do not open public issues for vulnerabilities or leaked credentials. Contact the maintainers privately through the repository owner's GitHub profile, and include enough detail to reproduce the problem.

## License

By contributing, you agree that your contributions are licensed under the [MIT License](LICENSE).
