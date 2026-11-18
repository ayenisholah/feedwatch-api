# Progress

**Updated:** 2026-11-18

Read this first. Update it in the same commit as the work. The system-wide state is in `../feedwatch-platform/docs/PROGRESS.md`.

## Next step

M2's API work is done. To close M2:

1. Tag `v0.1.0` (it must match `Cargo.toml`). `release.yml` publishes the release with `openapi.json`, the contract feedwatch-web pins in M3.
2. In feedwatch-platform: point `DATA_CONTRACT.md` at the release, add the `api` service to the local `compose.yaml`, and mark FR-FW-6, NFR-2 and GR-2 done.

Then M3 in feedwatch-web: `feat/api-client` against `openapi.json` v0.1.0.

## Done

| Date | Work |
|---|---|
| 2026-11-18 | `feat/cache-15s`: the data-route cache drops from 30 s to 15 s (`TTL_S`, `Cache-Control: public, max-age=15`) to match the poller's new 15 s interval (feedwatch-platform #25). The cache tests are written in terms of `TTL_S`. |
| 2026-11-18 | `fix/feed-id-chars` (FR-FW-6): 19 of the 146 manifest keys contain `+` or `/` (`CASH+_FUNDAMENTAL`, `WBTC/BTC`), and `valid_feed_id` rejected them, so their detail route was 404. Found while building the web feed detail page. `+` and `/` are now allowed; IDs are still escaped by `flux_string`. A client sends `/` as `%2F`, which axum decodes within the one path segment (`fr_fw_6_feed_detail_accepts_manifest_ids_with_slash_and_plus`). |
| 2026-11-18 | `fix/cache-get-only` (GR-2): a production check found that `POST /api/feeds` returned the cached GET body with 200. The cache layer wraps the whole route, including its 405 for other methods. Only GETs are now cached or served from the cache; regression test `gr_2_other_methods_are_405_even_when_the_get_is_cached`. |
| 2026-11-18 | `feat/limits` (GR-2):<br>- **GET only:** every route; other methods are 405.<br>- **CORS:** `tower-http` with `AllowOrigin::list`, so the header is sent only to `FEEDWATCH_ALLOWED_ORIGIN`; methods GET and OPTIONS.<br>- **Rate limit:** a token bucket per client IP (`src/limits.rs`), burst 20, refilled at 2 per second; over it is 429 with `Retry-After: 1`. The IP is the first `X-Forwarded-For` entry (set by Caddy), else the peer address (`into_make_service_with_connect_info`). At 10,000 tracked clients, full buckets are dropped.<br>- **Cache:** 30 s (`src/cache.rs`) on the feeds, compare and incidents routes, keyed by path and query. It keeps only 200s, holds at most 1,000 entries, and sends `Cache-Control: public, max-age=30`.<br>- **Time:** both use the injected clock. |
| 2026-11-18 | `feat/openapi` (FR-FW-6 contract): utoipa 6 generates an OpenAPI 3.1 document from handler annotations and schema derives (`ApiDoc`), served at `GET /api/openapi.json` and committed as `openapi.json`. `fr_fw_6_openapi_matches_committed` fails on drift (`UPDATE_OPENAPI=1` rewrites the file). `release.yml` publishes a GitHub release with `openapi.json` attached for each `v*` tag, after checking that the tag matches `Cargo.toml` and that the document is current. |
| 2026-11-18 | `feat/incidents` (FR-FW-6): `GET /api/incidents?status=open\|all` (default `open`; anything else is 400) returns `{incidents}`, newest first, at most 500. Each has `{id, chain, feedId, kind, openedAt, closedAt, detail}`. The query reads `incident_id` and `detail` over 30 days. `pair_incidents` joins each event's two fields by time and tags, and pairs open and close by ID; a close whose open is older than 30 days keeps `openedAt: null`. Tested with fake CSV (quotes and commas in details, kind changes, the cap) and a real InfluxDB. |
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
