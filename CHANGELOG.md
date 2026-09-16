# Changelog

All notable changes to this project are documented here.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and
the project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

- **Rebuilt the contracts around the mortgage product.** The initial four
  contracts modelled fractional property ownership, which is not what Stellar
  Homes is. The backend and frontend model a diaspora mortgage platform with
  milestone-gated disbursement, and the contracts now match them.

### Added

- `PropertyRegistryContract` — trustee-submitted properties, oracle title
  verification, surveyor valuations, and a five-stage construction schedule
  signed off in build order. A trustee can never verify or value their own
  property.
- `LendingPoolContract` — investor deposits, commitment of approved facilities,
  staged disbursement, and pro-rata interest with a carry so nothing is lost to
  truncation. Committed capital cannot be withdrawn.
- `MortgagePoolContract` — application, underwriting, milestone-gated release,
  repayment and default. Interest accrues monthly on the drawn balance, with a
  carry that makes accrual independent of how often it is triggered.
- Permissionless `disburse` and `mark_default`, both decided entirely by state
  the caller cannot influence.
- An 80% loan-to-value ceiling and a 30% rate cap, fixed in code rather than
  configuration.
- Two-step admin handover and timelocked upgrades on all three contracts.
- Deployment, multisig handover and testnet smoke scripts.
- CI: formatting, clippy at `-D warnings`, tests, wasm build, size regression
  budget, dependency audit and an 80% line coverage floor.
- Documentation covering architecture, authorization, the interest maths,
  storage and TTL policy, the threat model, deployment and events.

### Removed

- The fractional-ownership contracts: `sh-share-ledger`, `sh-offering` and
  `sh-income-distributor`.

[Unreleased]: https://github.com/NeonsLabs/stellar-homes-contract/commits/main
