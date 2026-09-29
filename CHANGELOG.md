# Changelog

## Unreleased — Milestone 2 discovery

- Added bounded Java 26.3 status/ping and Bedrock 1.26.51 UDP discovery paths with independent codec and process tests. Java server-list status was observed in a matching client; Bedrock real-client evidence remains open. Login and world remain absent.
- Added an independent, deterministic seeded terrain prototype and raw generation-only measurement utility. It is not yet sent to a client.

## Milestone 1

- Added a bounded loopback-only development listener, strict runtime configuration, lifecycle events, signal shutdown, and process integration tests.
- Added raw startup measurement tooling and a locked dependency-license CI gate. No playable world exists.

## Milestone 0 foundation

- Drafted independent architecture, compatibility boundaries, decisions, and contributor policies.
- Added one-package Rust bootstrap CLI with configuration validation, tests, and CI configuration.

No playable server or release exists.
