# feedwatch-api

[![CI](https://github.com/ayenisholah/feedwatch-api/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/ayenisholah/feedwatch-api/actions/workflows/ci.yml) [![Licence: MIT](https://img.shields.io/badge/licence-MIT-blue)](LICENSE) [![Rust 1.96](https://img.shields.io/badge/rust-1.96-orange)](rust-toolchain.toml)

The Feedwatch read API: Axum over the InfluxDB data the poller writes, serving the web client under `/api`.

Part of [Feedwatch](https://github.com/ayenisholah/feedwatch-platform/blob/main/SPEC.md). System docs, the data contract and the roadmap live in `feedwatch-platform`.

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

## Feedwatch repositories

| Repo                                                                    | What it is                                                                         |
| ----------------------------------------------------------------------- | ---------------------------------------------------------------------------------- |
| [feedwatch-platform](https://github.com/ayenisholah/feedwatch-platform) | Specification, requirements, ADRs, data contract, Compose, Grafana, AWS deployment |
| [feedwatch-poller](https://github.com/ayenisholah/feedwatch-poller)     | Rust worker: reads every feed, runs the checks, writes to InfluxDB                 |
| [feedwatch-api](https://github.com/ayenisholah/feedwatch-api)           | Rust (Axum) read API over InfluxDB                                                 |
| [feedwatch-web](https://github.com/ayenisholah/feedwatch-web)           | Next.js static client in the Frox design system                                    |

## Licence

MIT
