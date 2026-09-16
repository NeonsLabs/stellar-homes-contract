# Stellar Homes — Smart Contracts

> Diaspora mortgages for building back home, released against the building.

Somebody in the diaspora wants to build a house in Lagos, Accra or Nairobi. A
trustee on the ground registers the plot and its title; a land registry oracle
confirms the title is clean and a licensed surveyor values it. Investors put
capital into a lending pool. The borrower draws a mortgage against the
property — but the money is never handed over in one lump. It is released one
construction stage at a time, and only for a stage an inspector has signed off.

This repository holds the **Soroban smart contracts only** — the settlement
layer that custodies capital, releases it against verified progress, and tracks
what is owed. The KYC pipeline, the trustee portal and the borrower dashboard
live in `stellar-homes-backend` and `stellar-homes-frontend`.

## The Contracts

| Crate | Contract | Responsibility |
|-------|----------|----------------|
| `sh-property-registry` | `PropertyRegistryContract` | What a property is, who holds it in trust, whether its title is clean, what it is worth, and how far the build has got. Holds no money. |
| `sh-lending-pool` | `LendingPoolContract` | The investors' capital. Deposits, commitments, disbursement and yield. Holds every cent, and does not know what a mortgage is. |
| `sh-mortgage-pool` | `MortgagePoolContract` | The loan: application, underwriting, milestone-gated release, repayment and default. Decides what is owed and holds nothing. |

The contract holding the money is never the contract deciding whose it is.
Neither the lending pool nor the mortgage pool can, alone, pay the wrong party.

## How it works

1. **Registration.** A registered trustee submits a property: the digest of its
   title deed and the surveyor's report. The five construction stages —
   foundation, walls, roofing, finishing, handover — are created at the same
   time, so the schedule exists before anyone lends against it.
2. **Verification.** An oracle confirms the title against the land registry, and
   a surveyor publishes a valuation. A trustee can never verify or value their
   own property, even if the admin has granted them both roles.
3. **Capital.** Investors deposit into the lending pool and receive a claim
   proportional to what they put in.
4. **Application.** The borrower applies against the verified property. Nothing
   is committed; the loan cannot exceed 80% of the valuation, a ceiling fixed in
   code rather than configuration.
5. **Underwriting.** An underwriter approves, and the whole facility is
   **committed** against the pool's capital. It stops being withdrawable
   immediately, even though it will be released over months of building — a
   borrower whose foundation passes inspection should not find the money gone.
6. **Release.** The trustee submits evidence for a stage; an inspector signs it
   off; the tranche is released. Stages are signed off **in order**, so nobody
   draws the roofing tranche on a poured foundation. The money goes to the
   trustee running the build, never to the borrower.
7. **Repayment.** Interest accrues monthly on the balance actually drawn.
   Payments clear interest first, then principal. Paying more than the
   instalment shortens the loan; the loan can be cleared outright at any time.
8. **Default.** If an instalment runs unpaid past the grace period, anyone may
   write the loan off. The undrawn facility returns to the pool; the drawn
   balance falls on investors' capital, which is the risk they took.

### Interest, and why it is not the annuity the front end draws

Interest accrues monthly on the outstanding balance, and principal amortises in
equal instalments over the term — a **constant-amortisation** loan, where each
payment is the month's interest plus a fixed slice of principal, so the payment
falls as the balance does.

This is deliberately not the level-payment annuity the borrower dashboard
projects. An annuity needs `(1 + r)^n`, and there is no way to evaluate that in
integer arithmetic without either an approximation the borrower would have to
trust or a fixed-point exponent routine large enough to be its own audit
surface. Constant amortisation is exact in integers, so what the chain charges
can be checked by hand. **The contract is the source of truth for what is owed;
any schedule rendered elsewhere is a projection.**

Accrual carries its remainder rather than dropping it, so charging a month at a
time and charging six months at once come to exactly the same figure. Without
that, a borrower who poked the contract monthly would round down twelve times a
year and owe less than one who left it alone.

Interest is only ever charged on money actually disbursed, so a borrower whose
build has stalled at the foundation is not paying for a roof they have not got.

### What is deliberately not on-chain

No title deed, no survey, no photographs, no identity. A title deed names people
and places; publishing one would expose the borrower, the seller and the plot to
anyone who cared to look, permanently. The registry stores only 32-byte digests,
so the documents stay off-chain, are disclosed to the parties who need them, and
can still be proved unaltered by anyone holding a copy. KYC runs entirely in the
backend and never reaches the ledger.

### Trust boundary

The protocol custodies capital and releases it against signatures. It cannot see
a building. It does not know whether the foundation in the photographs is the
foundation of this plot, whether the surveyor's number is honest, or whether the
trustee will spend the tranche on the house. Those are the oracle's judgment and
the trustee's obligation.

What the protocol provides is procedure: money moves only in the order the build
is supposed to happen, only on a signature from somebody who is not the trustee,
and every signature is attributed on-chain to whoever gave it.

## Roles

| Role | Held by | May |
|------|---------|-----|
| **Trustee** | Registered address | Submit a property, submit milestone evidence, receive the tranches |
| **Oracle** | Registered address, never the trustee of the property in question | Verify titles, publish valuations, sign off construction stages |
| **Underwriter** | Registered address | Approve or decline applications |
| **Borrower** | Any Stellar wallet | Apply, repay, clear the loan early |
| **Investor** | Any Stellar wallet | Deposit, withdraw uncommitted capital, claim interest |
| **Admin** | Multisig account | Wire the contracts together once, register and revoke roles, schedule upgrades and grace-period changes behind the timelock, hand the admin role over |
| **Anyone** | — | Release a tranche for a signed-off stage; write off a loan past its grace period |

The admin has no path to anyone's money. There is no sweep, no forced transfer,
no way to mint a share of the pool, and no way to release a tranche the
inspector has not signed. The 80% loan-to-value ceiling and the 30% rate cap are
fixed in code, so moving them needs an upgrade and its timelock.

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

### Build, test, lint

```bash
make build      # release wasm
make test       # all suites
make all        # everything CI runs: fmt, clippy, tests, build
```

Use `wasm32v1-none`, not `wasm32-unknown-unknown`. Recent Rust enables wasm
features on the latter (`reference-types`) that the Soroban VM rejects, and the
failure only shows up at deploy time.

Artifacts land in `contracts/target/wasm32v1-none/release/`:

```
sh_property_registry.wasm
sh_lending_pool.wasm
sh_mortgage_pool.wasm
```

## Deploying

```bash
export USDC=<settlement asset contract id>
export TRUSTEE=<first trustee's address>
export ORACLE=<verification oracle's address>

./scripts/deploy.sh
```

It prints the three contract ids. Wiring between the contracts can be set only
once, so let the script run to completion — a half-wired deployment cannot be
repaired without an upgrade.

Then hand the admin role to a multisig and exercise the lifecycle end to end:

```bash
./scripts/handover-to-multisig.sh
./scripts/smoke-testnet.sh
```

See [docs/DEPLOYMENT.md](docs/DEPLOYMENT.md) for the full sequence.

## Documentation

| Document | What it covers |
|----------|----------------|
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | How the three contracts fit together and why the boundaries fall where they do |
| [docs/AUTHORIZATION.md](docs/AUTHORIZATION.md) | Every role, every privileged call, and what each one cannot reach |
| [docs/INTEREST_AND_REPAYMENT.md](docs/INTEREST_AND_REPAYMENT.md) | The interest and amortisation maths in full, with worked examples |
| [docs/STORAGE_TTL_POLICY.md](docs/STORAGE_TTL_POLICY.md) | Which storage each entry lives in and how it is kept alive |
| [docs/THREAT_MODEL.md](docs/THREAT_MODEL.md) | What the protocol defends against, and what it does not |
| [docs/DEPLOYMENT.md](docs/DEPLOYMENT.md) | Deploying, wiring, handing over and upgrading |
| [docs/EVENTS.md](docs/EVENTS.md) | Every event, for indexers |

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Security issues go to
[SECURITY.md](SECURITY.md), not the public issue tracker.

## Licence

MIT. See [LICENSE](LICENSE).
