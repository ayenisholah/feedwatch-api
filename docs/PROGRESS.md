# Progress

**Updated:** 2026-11-16

Read this first. Update it in the same commit as the work. The system-wide state is in `../feedwatch-platform/docs/PROGRESS.md`.

## Next step

Wait until the poller writes real data (M1). Then branch `feat/influx-client` (FR-FW-6): a Flux client that posts to `/api/v2/query` and parses annotated CSV, tested against `testcontainers` InfluxDB seeded with points that follow feedwatch-platform `contracts/influx-schema.json`.

After that: `feat/feeds-routes`, `feat/compare`, `feat/incidents`, `feat/openapi` (utoipa; publish `openapi.json` with each release) and `feat/limits` (GR-2: CORS, rate limit, cache).

## Done

| Date | Work |
|---|---|
| 2026-11-16 | Scaffold: Axum app with `/api/health` and tests (`nfr_2_health_reports_ok`, `gr_2_unknown_routes_are_not_found`), module stubs, check script, Dockerfile, CI. |

## In progress

Nothing.

## Blocked

Nothing.

## Notes

- `cargo-deny` isn't installed locally; CI installs it. Install it with `cargo install cargo-deny --locked`.
