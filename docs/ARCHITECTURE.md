# Architecture

Three contracts, each with one job. The split is what keeps any single contract
from being able to both decide who is owed something and move it.

```
   trustee ──────┐
   oracle ───────┤
                 ▼
        ┌────────────────────────┐
        │   PropertyRegistry     │   titles, valuations,
        │                        │   five-stage build schedule
        │      holds no money    │
        └───────────┬────────────┘
           reads    │    writes
        terms and   │    release + status
        sign-offs   │
        ┌───────────▼────────────┐
 borrower ─────────▶│  MortgagePool          │   the loan: terms, balance,
 (apply, repay)     │                        │   what is owed
 anyone ───────────▶│      holds no money    │
 (release, default) └───────────┬────────────┘
                     instructs  │
        ┌───────────────────────▼┐
 investor ─────────▶│   LendingPool          │──── tranche ───▶ trustee
 (deposit, claim)   │                        │──── interest ──▶ investor
                    │   holds every cent     │
                    └────────────────────────┘
```

## Why the boundaries fall here

**Money and accounting are separated.** The MortgagePool knows every loan's
balance, rate and schedule, and holds nothing. The LendingPool holds every cent
and has no idea what a mortgage is — it is told how much to reserve, where to
send a tranche, and how much of an arriving payment is interest. Neither alone
can pay the wrong party the wrong amount.

**Verification and value are separated.** The registry records whether a title
is clean and whether a construction stage has passed inspection, but has no
route to any money. A compromised registry can lie about a building; it cannot
spend against one. That is precisely why the sign-offs the MortgagePool relies
on live there.

**Commitment and custody are separated in time.** An approved mortgage commits
its whole facility against the pool immediately, but the money moves months
later, a stage at a time. Reserving up front is what stops a pool approving a
mortgage, letting investors withdraw, and then being unable to pay the roofing
tranche of a half-built house.

## The call graph

Every cross-contract call is one-directional, and the graph is acyclic:

| Caller | Callee | Calls | Why it is allowed |
|--------|--------|-------|-------------------|
| MortgagePool | PropertyRegistry | `lending_terms`, `is_releasable`, `mark_released`, `mark_mortgaged`, `mark_repaid`, `mark_defaulted` | The registry stores the pool's address once, at setup, and accepts these from nothing else |
| MortgagePool | LendingPool | `reserve`, `unreserve`, `disburse`, `repay`, `write_off` | The pool stores the mortgage pool's address once, at setup |

Because there is no cycle, no call can re-enter the contract it started from.
The one place that would otherwise be exposed — `disburse`, which checks a stage
is releasable and then moves money — marks the stage released **before** the
transfer, so even a re-entrant registry would find nothing left to draw.

Calls are made as clients declared with `#[contractclient]` rather than as crate
dependencies, so no contract's code is linked into another's wasm. The registry
exposes `lending_terms`, which returns plain values instead of a `Property`, and
three separate `mark_*` callbacks instead of one taking a status enum —
precisely so the two contracts share no types across the wire.

## Permissionless by design

Two operations are open to anyone, because in both cases what the call does is
fixed entirely by state the caller cannot influence:

- **`disburse`** — the registry decides whether a stage is signed off and
  unpaid, and the tranche size is fixed by the loan. A trustee should not have
  to wait on the platform to press a button once the inspector has signed.
- **`mark_default`** — the loan's own arrears and the grace period decide it. The
  platform cannot keep a bad loan off the books by declining to act on it.

## Wiring, and why it is one-shot

Two addresses can only be learned after deployment, because the contract that
owns them does not exist when its counterparts are constructed:

```
registry.set_mortgage_pool(mortgage_pool)
lending.set_mortgage_pool(mortgage_pool)
```

Each can be set exactly once. Rewiring later would let an admin point the
treasury at a contract that pays itself, or the registry at one that marks any
stage funded. Changing them is therefore an upgrade, and goes through the
upgrade timelock where investors and borrowers can see it coming.

The practical consequence is that a half-wired deployment cannot be repaired.
`scripts/deploy.sh` does the whole sequence in one run for that reason.

## Constructors, not initializers

Every contract sets its configuration in `__constructor`, which runs atomically
inside the deploy transaction. There is no separate `initialize` call, so there
is no window between deployment and setup in which somebody else could claim the
admin role.

## Administration, uniformly

All three contracts share the same administrative shape:

- **Two-step handover.** `propose_admin` changes nothing; the named address must
  `accept_admin` from its own account. A mistyped address cannot lock the
  protocol out.
- **Timelocked sensitive changes.** Upgrades, and on the MortgagePool the grace
  period, are scheduled and execute only after a delay fixed at deployment. One
  action may be pending at a time, so a queued change stays visible, and it can
  be cancelled before it runs.
- **Nothing that reaches user funds.** No admin call transfers the settlement
  asset, mints a claim on the pool, releases a tranche or alters a loan balance.
  The 80% loan-to-value ceiling and the 30% rate cap are constants in code, not
  configuration.

## Storage

Every long-lived record — properties, milestones, mortgages, investor positions
— is in **persistent** storage and extended on every use. Configuration, wiring
and the pool's running totals are in **instance** storage, extended on every
call, so a contract's code and the configuration it needs expire together or not
at all. See [STORAGE_TTL_POLICY.md](STORAGE_TTL_POLICY.md).
