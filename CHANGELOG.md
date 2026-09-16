# Changelog

All notable changes to this project are documented here.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and
the project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `PropertyRegistryContract` — property records, sponsor and appraiser roles,
  independent valuations, and the `Draft → Offering → Owned | Failed → Retired`
  lifecycle.
- `ShareLedgerContract` — per-property cap table with share balances, transfers,
  and scaled per-share income accrual that survives holders trading between
  deposits.
- `OfferingContract` — primary sale with subscription escrow, a soft cap,
  permissionless close, pull-based share delivery and full refunds on failure.
- `IncomeDistributorContract` — custody of rental income with pull-based
  pro-rata claims.
- Two-step admin handover and timelocked upgrades on all four contracts, with
  the protocol fee capped at 10% in code.
- Deployment, multisig handover and testnet smoke scripts.
- CI: formatting, clippy at `-D warnings`, tests, wasm build, size regression
  budget, dependency audit and an 80% line coverage floor.
- Documentation covering architecture, authorization, income accounting, storage
  and TTL policy, the threat model, deployment and events.

[Unreleased]: https://github.com/NeonsLabs/stellar-homes-contract/commits/main
