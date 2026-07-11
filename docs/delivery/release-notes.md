# Delivery Release Notes

## Current Release State

- iteration-1 establishes PROQAID governance and delivery baselines only.
- No production code, Phase 0 implementation, successful build, runnable environment, migration, deployment, or release artifact is claimed.
- The first GitHub version is a documentation/source-layout baseline, not an application release.

## Delivery Baseline

- Development and acceptance target: Ubuntu 24.04 LTS x86_64.
- Development database: PostgreSQL 16. Sovereign target: openGauss in a later dedicated adaptation phase.
- Intended stack: pinned Rust stable/Edition 2024, Python 3.12 with `uv`, C++20 with Clang 18/CMake/Ninja, TypeScript/React with Node.js 22/pnpm/Vite, Protobuf 3, Docker Compose, MinIO, and gVisor.
- Intended observability: structured Rust `tracing` events via OTLP with request, experiment, agent, node, artifact, user, and app correlation identifiers.
- Intended release evidence includes reproducible builds, locks and image digests, migrations, test results, SBOM and license inventory, vulnerability/dependency checks, and provenance.

## Current Readiness and Gaps

- Build, startup, deployment, configuration, migrations, observability, rollback/recovery, and release packaging are not yet implemented or verified.
- A later human-approved engineering iteration must provide command output and artifacts before any readiness or release claim.
- Project license, contribution workflow, release cadence, versioning policy, and operations manual remain to be established before external release.

## External Systems and Secrets

- Private GitHub repository `https://github.com/kayz/ficant` was created for the allowlisted first-version baseline.
- The GitHub baseline contains only `.gitignore`, `README.md`, `src/`, `docs/`, and `result/`; all other local project/governance material is excluded by `.gitignore`.
- First content commit `affce937b30ba14b59777691ec8d311dbb5161ba` was pushed to `main` and matched the remote commit and 10-file tree.
- Test host `47.100.66.40` is recorded as unverified/unused context; no connection or deployment occurred.
- Key directory `C:\git\key` is recorded as unverified/unused context; it was not read or enumerated.

## Required Authorization for Later Work

Any GitHub remote action, test-host connection/deployment, or use of a specifically named key path requires an explicit checklist item and task authorization. A worker must never enumerate the key directory to choose a credential.

## Validity

Valid: long-term until superseded
