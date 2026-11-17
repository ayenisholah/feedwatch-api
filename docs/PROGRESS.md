# Progress

**Updated:** 2026-11-18

Read this first. Update it in the same commit as the work. The system-wide state is in `../feedwatch-platform/docs/PROGRESS.md`.

## Next step

Branch `feat/feeds-routes` (FR-FW-6, NFR-2), following `../feedwatch-platform` M2 in `docs/ROADMAP.md`:

- `routes::router(state)` takes an `AppState` holding `Arc<dyn InfluxQuery>`; route tests use a fake store.
- `GET /api/health` returns `{status, influx, lastPollAt}` within 1 s.
- `GET /api/feeds?chain=` returns the latest `feed_round` and `feed_check` per feed; an unknown chain is 400.
- `GET /api/feeds/{chain}/{feedId}` returns latest, triggers, reference comparison and a 24 h series (5 min windows); an unknown feed is 404.

After that: `feat/compare`, `feat/incidents`, `feat/openapi` and `feat/limits` (GR-2).

## Done

| Date | Work |
|---|---|
| 2026-11-18 | `ci/deploy-on-merge` (NFR-3): a `Deploy` workflow calls feedwatch-platform's reusable deploy workflow after `CI` passes on `main`, so every merge reaches production (platform ADR 0008). Needs the `AWS_ACCESS_KEY_ID` and `AWS_SECRET_ACCESS_KEY` secrets from platform `scripts/aws/set-deploy-secrets.sh`. |
| 2026-11-17 | `feat/influx-client` (FR-FW-6): `Config::from_env` validates the listen address, InfluxDB settings and `FEEDWATCH_ALLOWED_ORIGIN`. `HttpInflux` implements `InfluxQuery`: it posts Flux to `/api/v2/query`, maps failures to `InfluxError` (unreachable, HTTP status, Flux error table), and parses annotated CSV, including quoted cells with commas, quotes and line breaks. Tested with wiremock and a real InfluxDB 2.9.1 (testcontainers, run when `CI` is set). Contract v2 vendored at `tests/fixtures/influx-schema.json` and checked against a sibling platform checkout. `cargo deny` ignores RUSTSEC-2026-0440 (dev-only, via testcontainers), as the poller does. |
| 2026-11-16 | Scaffold: Axum app with `/api/health` and tests (`nfr_2_health_reports_ok`, `gr_2_unknown_routes_are_not_found`), module stubs, check script, Dockerfile, CI. |

## In progress

Nothing.

## Blocked

Nothing.

## Notes

- The API now refuses to start without `INFLUX_URL`, `INFLUX_ORG`, `INFLUX_BUCKET`, `INFLUX_TOKEN` and `FEEDWATCH_ALLOWED_ORIGIN`. Before the next AWS deploy of the API, add `FEEDWATCH_ALLOWED_ORIGIN` to the host's `/opt/feedwatch/.env` (from SSM), or the container will exit.

- `cargo-deny` isn't installed locally; CI installs it. Install it with `cargo install cargo-deny --locked`.
