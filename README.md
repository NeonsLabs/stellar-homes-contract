# Stellar Homes — Smart Contracts

> Fractional ownership of real property, settled on Stellar.

A property is registered by a sponsor, valued by an independent appraiser, and
divided into shares. Investors buy those shares in a primary offering paid for
in USDC. Once the raise settles, the building belongs to its shareholders: rent
collected off-chain is paid into the protocol and split across every share, and
shares can change hands afterwards without anyone losing income they have
already earned.

This repository holds the **Soroban smart contracts only** — the settlement
layer that custodies money and records who owns what. The sponsor portal, the
property onboarding pipeline and the investor dashboard live in their own
repositories.

## The Contracts

| Crate | Contract | Responsibility |
|-------|----------|----------------|
| `sh-property-registry` | `PropertyRegistryContract` | What a property is, who sponsors it, what appraisers say it is worth, and where it sits in its life. Holds no money and issues no shares. |
| `sh-share-ledger` | `ShareLedgerContract` | The cap table, one property at a time: balances, transfers, and the running per-share income accounting. Holds no money. |
| `sh-offering` | `OfferingContract` | The primary sale. Escrows subscriptions, settles the raise above its soft cap or unwinds it below, pays the sponsor and the treasury. |
| `sh-income-distributor` | `IncomeDistributorContract` | Custody of rental income, and the window shareholders claim it through. Does no arithmetic about who is owed what. |

The contract holding the money is never the contract deciding whose it is. The
ShareLedger works out entitlements but cannot move a cent; the
IncomeDistributor moves money but only for the amount the ledger hands back.

## How it works

1. **Registration.** A registered sponsor records a property: the digest of its
   off-chain prospectus and title, how many shares it divides into, and what a
   share costs. It starts in `Draft` and cannot be sold.
2. **Appraisal.** A registered appraiser — never the sponsor, even if the admin
   granted them both roles — publishes a valuation. Only then may the sponsor
   open the offering.
3. **The raise.** Investors subscribe, paying into escrow. Nothing is issued and
   nothing reaches the sponsor while the sale is open; an investor may take
   their subscription back at any time before the deadline.
4. **Closing.** The sale closes when it sells out, or when its deadline passes.
   Anyone may close it — what closing does is fixed entirely by the sale's own
   state, so a sponsor cannot strand subscribers by refusing to close a raise
   that failed. Above the soft cap it settles: the sponsor is paid for the
   shares actually sold, the protocol fee goes to the treasury, and the registry
   records the property as `Owned`. Below it, the raise is unwound and every
   subscriber can take their money back in full.
5. **Delivery.** Shares and refunds are pulled, not pushed — a contract cannot
   loop over every subscriber, so each investor claims their own.
6. **Income.** Rent collected off-chain is deposited for the property and spread
   across every share issued at that moment. Holders claim whenever they like,
   however many months have piled up.
7. **Secondary market.** Shares transfer holder to holder. Both sides' income is
   settled first, so selling never forfeits income already earned and buying
   never lays claim to income earned before the purchase.

### Splitting income without iterating over holders

A contract cannot loop over every shareholder to pay them, so entitlement is
accrued rather than pushed. Each property carries a running `acc_per_share`:
everything ever deposited for it, divided by its issued shares, scaled by 10^12
to survive integer division. A holder's position records what that accumulator
was worth against their balance the last time it was settled; what they are owed
is the growth since, times their balance. Every balance change settles first,
banking what the old balance earned before re-anchoring against the new one.

Rent that does not divide evenly is not dropped. The remainder is kept in the
accumulator's own scaled units and folded into the next deposit, so truncation
never compounds and the contract can always cover what it owes.

### What is deliberately not on-chain

No street address, no deed, no tenant and no rent roll. The registry stores only
the 32-byte digest of the off-chain prospectus, deed and title report, so the
paperwork can be published, mirrored and verified anywhere without putting it on
a public ledger. Rent is collected in the local economy by a managing agent and
arrives as a single deposit; the protocol never sees a tenant.

### Trust boundary

The protocol custodies money and records ownership. It does not verify that a
building exists, that its title is clean, or that the rent deposited matches
what was collected. Those are the sponsor's obligations and the appraiser's
judgment, and the protocol's answer to them is procedural: a property cannot
reach the market without a valuation signed by somebody other than its sponsor,
and every valuation is attributed on-chain to the appraiser who published it, so
a stale or flattering number can be traced back to whoever signed it.

## Roles

| Role | Held by | May |
|------|---------|-----|
| **Sponsor** | Registered address | Register a property, open its offering, receive the proceeds, pay rent in |
| **Appraiser** | Registered address, never the sponsor of the property being valued | Publish valuations |
| **Investor** | Any Stellar wallet | Subscribe, withdraw before the deadline, claim shares or a refund, transfer shares, claim income |
| **Admin** | Multisig account (2-of-3 on testnet) | Wire the contracts together once, register and revoke sponsors and appraisers, retire an owned property, schedule upgrades and fee or treasury changes behind the timelock, hand the admin role over |
| **Anyone** | — | Close a sale once its conditions are met; what closing does is fixed by the sale's state |

The admin has no path to anyone's money. There is no sweep, no forced transfer
and no way to mint shares: unclaimed income stays claimable by its owner
indefinitely.

## Stack

* **Language:** Rust (edition 2021), `#![no_std]`
* **SDK:** Soroban SDK v22
* **Build target:** `wasm32v1-none`
* **Settlement asset:** USDC via the Stellar Asset Contract

## Running it locally

### Prerequisites

* Rust (latest stable)
* WASM target — `rustup target add wasm32v1-none`
* Stellar CLI — `cargo install --locked stellar-cli`, or a prebuilt release

### Build

```bash
make build
```

Use `wasm32v1-none`, not `wasm32-unknown-unknown`. Recent Rust enables wasm
features on the latter (`reference-types`) that the Soroban VM rejects, and the
failure only shows up at deploy time.

Artifacts land in `contracts/target/wasm32v1-none/release/`:

```
sh_property_registry.wasm
sh_share_ledger.wasm
sh_offering.wasm
sh_income_distributor.wasm
```

### Test

```bash
make test                                              # all suites
cargo test --manifest-path contracts/Cargo.toml -p sh-offering   # one crate
```

### Everything CI runs

```bash
make all      # fmt-check, clippy, tests, build
```

## Deploying

```bash
export USDC=<settlement asset contract id>
export SPONSOR=<first sponsor's address>
export APPRAISER=<appraiser's address>

./scripts/deploy.sh
```

It prints the four contract ids. Wiring between the contracts can be set only
once, so let the script run to completion rather than stopping midway — a
half-wired deployment cannot be repaired without an upgrade.

Then hand the admin role to a multisig and verify the property lifecycle end to
end against testnet:

```bash
./scripts/handover-to-multisig.sh
./scripts/smoke-testnet.sh
```

See [docs/DEPLOYMENT.md](docs/DEPLOYMENT.md) for the full sequence.

## Documentation

| Document | What it covers |
|----------|----------------|
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | How the four contracts fit together and why the boundaries fall where they do |
| [docs/AUTHORIZATION.md](docs/AUTHORIZATION.md) | Every role, every privileged call, and what each one can and cannot reach |
| [docs/INCOME_DISTRIBUTION.md](docs/INCOME_DISTRIBUTION.md) | The accrual accounting in full, with worked examples |
| [docs/STORAGE_TTL_POLICY.md](docs/STORAGE_TTL_POLICY.md) | Which storage each entry lives in and how it is kept alive |
| [docs/THREAT_MODEL.md](docs/THREAT_MODEL.md) | What the protocol defends against, and what it does not |
| [docs/DEPLOYMENT.md](docs/DEPLOYMENT.md) | Deploying, wiring, handing over and upgrading |
| [docs/EVENTS.md](docs/EVENTS.md) | Every event, for indexers |

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Security issues go to
[SECURITY.md](SECURITY.md), not the public issue tracker.

## Licence

MIT. See [LICENSE](LICENSE).
