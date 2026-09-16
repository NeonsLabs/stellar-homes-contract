# Threat model

What the protocol defends against, how, and — just as importantly — what it does
not.

## What is being protected

1. **Investor capital** in the lending pool.
2. **Committed facilities** — that money promised to a build is there when the
   inspector signs.
3. **Tranches** — that released money reaches the build, in build order.
4. **Loan balances** — that what a borrower owes is computable and cannot be
   inflated or quietly forgiven.

## Trust assumptions

The protocol assumes:

- The Stellar network orders and finalizes transactions correctly.
- The settlement asset's Stellar Asset Contract behaves as a standard token.
- The admin multisig's signers are not all compromised at once.
- **Registered oracles are honest.** This is the big one, and it is discussed
  below rather than assumed away.

It does **not** assume trustees are honest, borrowers will pay, or that anyone
will call `disburse` or `mark_default` promptly.

## Threats and mitigations

### A trustee takes a tranche and does not build

Not prevented on-chain, and not claimed to be. The protocol releases money
against an inspector's signature; it cannot see a building.

What it does provide: money is released in build order, a stage at a time, so
the most a dishonest trustee can take before anyone notices is one tranche —
and only after passing an inspection they did not sign. The remaining facility
stays committed in the pool, and is returned when the loan defaults.

### A trustee signs off their own work

Refused. `verify_title`, `set_valuation` and `verify_milestone` all reject a call
from the property's own trustee, checked against the property record rather than
the role flag — so granting one address both roles does not open a hole.

### A corrupt or careless oracle

The residual risk. An oracle that signs off a foundation that does not exist, or
values a plot at three times its worth, can cause real loss. The admin can revoke
an oracle, but only going forward.

Mitigations are procedural rather than cryptographic: every valuation and every
sign-off is attributed on-chain to the address that gave it, permanently; the
80% loan-to-value ceiling caps what an inflated valuation can extract; and
release in build order means a single bad signature releases one tranche, not
the facility. Operators should treat oracle vetting and key custody as the real
control, and use distinct oracles for valuation and inspection where possible.

### The same property backing two loans

Refused. A property has at most one live mortgage, tracked by
`PropertyMortgage`. It is freed when the loan is declined, paid off or
defaulted — never while it is live.

### Drawing a tranche twice

The registry marks a stage `released`, and `disburse` marks it **before** the
money moves. Even a re-entrant registry would find the stage already drawn.

### Drawing out of order

Stages are signed off in order: `verify_milestone` refuses stage *n* until stage
*n − 1* has passed. Evidence for a later stage can be submitted early, but it
cannot be signed off, and so cannot be drawn.

### Investors withdrawing capital promised to a build

Refused. An approved mortgage reserves its whole facility immediately, and
`withdraw` can only touch uncommitted capital. A borrower whose foundation
passes cannot find the money gone.

### A borrower minimising interest by timing their calls

Prevented by the accrual carry. Charging a month at a time and charging six
months at once come to exactly the same figure; a test asserts it. Without it a
borrower who poked the contract monthly would round down twelve times a year.

### A borrower stalling with token payments

`repay` requires either the full instalment or the whole payoff. A payment in
between is refused, so nothing can look like a payment while leaving the
schedule where it was.

### The platform hiding a bad loan

`mark_default` is permissionless once arrears run past grace. The platform
cannot keep a defaulted loan on the books by declining to act on it, and an
investor can crank it themselves.

### The platform stalling a legitimate build

`disburse` is permissionless once a stage is signed off. The trustee does not
need the platform's cooperation to draw a tranche an inspector has approved.

### A rogue admin

The admin cannot move the settlement asset, mint a claim on the pool, alter a
loan, release a tranche, or claim on anyone's behalf. No such entry point
exists. What the admin *can* do:

| Power | Constraint |
|-------|-----------|
| Upgrade any contract | Timelocked, cancellable, visible while queued |
| Change the grace period | Timelocked; zero is refused |
| Register or revoke trustees, oracles, underwriters | Forward-looking only |

An upgrade is unrestricted in what it could introduce — that is the nature of
upgradeable code. The timelock is what makes it survivable: investors can see a
queued upgrade and withdraw uncommitted capital before it executes. Deploy with a
timelock long enough for that to be a real option.

### Admin key loss

`accept_admin` requires the proposed address to sign for itself, so a typo cannot
hand the role to an address nobody controls. The role cannot be renounced.

If the key is lost outright the contracts keep working — every user-facing path
is unaffected — but can never be upgraded or have roles changed again.

### Arithmetic

`overflow-checks` is left on in the release profile, so any overflow panics
rather than wrapping. Interest carries its remainder; the pool carries its
interest remainder; both are covered by tests asserting the contracts never owe
more than they hold.

## Out of scope

- Whether a building exists, or matches its photographs
- Whether a title is genuinely clean, beyond the oracle's word
- Whether a trustee spends a tranche on the house
- Off-chain identity, KYC and sanctions screening
- Tax, and the enforceability of the mortgage in local law
- Recovery of a defaulted property — foreclosure is entirely off-chain
