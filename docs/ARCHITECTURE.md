# Architecture

Four contracts, each with one job. The split is not cosmetic: it is what keeps
any single contract from being able to both decide who owns something and move
it.

```
                       ┌───────────────────────┐
                       │   PropertyRegistry    │
                       │                       │
     sponsor ─────────▶│  properties, roles,   │
     appraiser ───────▶│  valuations, status   │
                       └───────────┬───────────┘
                          reads terms │ writes outcome
                                      │
                       ┌──────────────▼────────┐
    investor ─────────▶│       Offering        │──── USDC ───▶ sponsor
                       │                       │──── fee ────▶ treasury
                       │  subscription escrow  │
                       └───────────┬───────────┘
                                   │ issue
                       ┌───────────▼───────────┐
    holder ───────────▶│      ShareLedger      │
    (transfer)         │                       │
                       │  balances + accrual   │
                       └───────────▲───────────┘
                        accrue │   │ take_accrued
                               │   │
                       ┌───────┴───▼───────────┐
    holder ───────────▶│   IncomeDistributor   │──── USDC ───▶ holder
    (claim)            │                       │
    agent ────────────▶│   custody of rent     │
    (deposit)          └───────────────────────┘
```

## Why the boundaries fall here

**Money and accounting are separated.** The ShareLedger knows who held which
shares when, so it is the only thing that can work out who is owed what. It
holds no money at all. The IncomeDistributor holds every cent of rent but does
no arithmetic: it asks the ledger what a holder is owed and pays exactly that.
Neither contract alone can pay the wrong person the wrong amount.

**Custody and ownership are separated.** During a raise the Offering contract
holds subscriptions and the ShareLedger holds nothing, because no shares exist
yet. At settlement the two swap roles: the money leaves, the shares appear. An
investor's claim on one is never simultaneously a claim on the other.

**Identity and value are separated.** The registry records what a property is
and who speaks for it, but has no route to any money and cannot issue a share.
A compromised registry can mislabel a property; it cannot spend one.

## The call graph

Every cross-contract call is one-directional, and the graph is acyclic:

| Caller | Callee | Calls | Why it is allowed |
|--------|--------|-------|-------------------|
| Offering | PropertyRegistry | `offering_terms`, `mark_owned`, `mark_failed` | The registry stores the Offering's address once, at setup, and accepts status changes from nothing else |
| Offering | ShareLedger | `issue` | The ledger stores the Offering's address once, at setup, and mints for nothing else |
| IncomeDistributor | ShareLedger | `accrue`, `take_accrued`, `accrued_of` | The ledger stores the distributor's address once, at setup |

Because there is no cycle, no call can re-enter the contract it started from.
The one place that would otherwise be exposed — `claim_shares`, which zeroes a
subscription and then calls the ledger — zeroes first, so even a re-entrant
ledger would find nothing left to claim.

Calls are made as clients declared with `#[contractclient]` rather than as crate
dependencies, so no contract's code is linked into another's wasm. The registry
exposes `offering_terms`, which returns plain values instead of a `Property`,
precisely so the Offering contract does not need the registry's types.

## Wiring, and why it is one-shot

Three addresses can only be learned after deployment, because the contracts
that own them do not exist when their counterparts are constructed:

```
registry.set_offering(offering)
ledger.set_offering(offering)
ledger.set_distributor(distributor)
```

Each can be set exactly once. Rewiring later would let an admin point the ledger
at a contract that mints itself a majority of any property, or point the registry
at one that marks any property owned. Changing them is therefore an upgrade, and
goes through the upgrade timelock where shareholders can see it coming.

The practical consequence is that a half-wired deployment cannot be repaired.
`scripts/deploy.sh` does the whole sequence in one run for that reason.

## Constructors, not initializers

Every contract sets its configuration in `__constructor`, which runs atomically
inside the deploy transaction. There is no separate `initialize` call, so there
is no window between deployment and setup in which somebody else could claim the
admin role.

## Administration, uniformly

All four contracts share the same administrative shape:

- **Two-step handover.** `propose_admin` changes nothing; the named address must
  `accept_admin` from its own account. A mistyped address cannot lock the
  protocol out.
- **Timelocked sensitive changes.** Upgrades, and on the Offering contract the
  treasury and fee, are scheduled and execute only after a delay fixed at
  deployment. One action may be pending at a time, so a queued change stays
  visible, and it can be cancelled before it runs.
- **Nothing that reaches user funds.** No admin call transfers the settlement
  asset, mints a share, or moves a holder's position. The protocol fee is
  capped in code at 10%, not in configuration, so raising it past that ceiling
  needs a code upgrade rather than a parameter change.

## Storage

Every long-lived record — properties, positions, sales, subscriptions, per
property totals — is in **persistent** storage and extended on every use.
Configuration and wiring are in **instance** storage, extended on every call, so
the contract's code and its configuration expire together or not at all. See
[STORAGE_TTL_POLICY.md](STORAGE_TTL_POLICY.md).
