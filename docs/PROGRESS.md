# Progress

**Updated:** 2026-11-18

Read this first. Update it in the same commit as the work. The system-wide state is in `../feedwatch-platform/docs/PROGRESS.md`.

## Next step

Branch `feat/incidents` (FR-FW-6): `GET /api/incidents?status=open|all` (default `open`; anything else gives 400).

- Flux over `range(-30d)` on `_measurement == "incident"`, fields `incident_id` and `detail`, with the `chain`, `feed_id`, `kind`, `event` and `_time` columns.
- A pure `pair_incidents(rows)` groups by `incident_id`. The `open` event gives `openedAt` and `detail`; `close` gives `closedAt`. Newest first, capped at 500. `open` keeps only incidents with no `closedAt`, matching the poller's `open_incidents_query`.
- Tests: an open incident, an open-and-close pair, a kind change (two IDs), details with commas, `all` against `open`, and a real-InfluxDB test.

After that: `feat/openapi` and `feat/limits` (GR-2).

## Done

| Date | Work |
|---|---|
| 2026-11-18 | `feat/compare` (FR-FW-6): `GET /api/compare` returns `{lastPollAt, feeds}` for the feeds present on all three chains. Each has `chains.{ethereum,arbitrum,base}` with `{value, ageS, status}`, and `spreadPct` (range over median of the readable values; `null` with fewer than two). `spread` is copied from the poller with its test, so both repos use one definition. |
| 2026-11-18 | `feat/feeds-routes` (FR-FW-6, NFR-2). `GET /api/health` returns `{status, influx, lastPollAt}`: it waits at most 800 ms for InfluxDB and reports `degraded` if InfluxDB is down or the last poll is more than 180 s old. `GET /api/feeds?chain=` returns `{lastPollAt, feeds}`, with each feed's latest round and check; an unknown chain is 400. `GET /api/feeds/{chain}/{feedId}` returns `{latest, series}`, a 24 h series in 5-minute windows; 404 if there's no data, 400 for a bad chain. Flux takes `last()` per series and `latest_by_feed` keeps only each feed's newest point, so an older status or optional field can't leak in. Feed IDs are validated and every Flux value is escaped. InfluxDB failures are 503 with a generic message. Routes take `AppState` (store, bucket, clock); tests use a fake store, plus a real InfluxDB 2.9.1 test that seeds two cycles. |
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
