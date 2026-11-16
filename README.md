# feedwatch-api

The Feedwatch read API: Axum over the InfluxDB data the poller writes, serving the web client under `/api`.

Part of [Feedwatch](../feedwatch-platform/SPEC.md). System docs, the data contract and the roadmap live in `feedwatch-platform`.

> **Status:** Scaffolded (M0). Only `GET /api/health`. See [progress](docs/PROGRESS.md).

## Run

```sh
cp .env.example .env
cargo run                # serves on FEEDWATCH_API_ADDR (default 0.0.0.0:8080)
curl localhost:8080/api/health
```

## Check

```sh
./scripts/check.sh       # fmt, clippy -D warnings, tests, coverage ≥95% (cargo-llvm-cov), cargo-deny
docker build -t feedwatch-api .
```

## Licence

MIT
