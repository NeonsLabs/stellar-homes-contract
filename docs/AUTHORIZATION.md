# Authorization

Who may call what, and what each caller can reach. Every entry point is listed;
anything not here is a getter and open to anyone.

## Roles

| Role | How it is held | Granted by | Revoked by |
|------|----------------|-----------|-----------|
| Admin | One address per contract, set at deploy | Two-step handover | Handing it on; it cannot be renounced |
| Trustee | Flag in the registry | `set_trustee(.., true)` | `set_trustee(.., false)`, effective next call |
| Oracle | Flag in the registry | `set_oracle(.., true)` | `set_oracle(.., false)`, effective next call |
| Underwriter | Flag in the mortgage pool | `set_underwriter(.., true)` | `set_underwriter(.., false)`, effective next call |
| Borrower / investor | Any wallet | — | — |
| MortgagePool | Address stored once in the registry and the lending pool | `set_mortgage_pool`, once | Only by upgrade |

Revocation is forward-looking. A revoked trustee keeps the properties they
already registered and any tranche already paid; they simply cannot submit or
draw anything new.

## PropertyRegistry

| Call | Who | Notes |
|------|-----|-------|
| `set_mortgage_pool` | Admin | Once only |
| `set_trustee`, `set_oracle` | Admin | Reversible |
| `schedule_action` / `execute_action` / `cancel_action` | Admin | Upgrade only, behind the timelock |
| `propose_admin` | Admin | Changes nothing on its own |
| `accept_admin` | The proposed address | Signs for itself |
| `submit_property` | Registered trustee | Creates the five build stages |
| `submit_milestone_evidence` | The property's **own** trustee | Replaceable until the stage is signed off |
| `verify_title` | Registered oracle, **never** the property's trustee | Pending → Verified |
| `set_valuation` | Registered oracle, **never** the property's trustee | Requires a verified title; may be refreshed |
| `verify_milestone` | Registered oracle, **never** the property's trustee | Needs evidence, and the previous stage signed off |
| `mark_released` | The wired MortgagePool | Only for a verified, unreleased stage |
| `mark_mortgaged` / `mark_repaid` / `mark_defaulted` | The wired MortgagePool | Each only from the one status that permits it |

The trustee checks are against the **property's own trustee**, not merely "is a
trustee". One registered trustee cannot submit evidence for another's property,
and granting one address both the trustee and oracle roles does not let it
verify its own work — that is checked against the property record, not the role
flag.

Stages are verified in order. Skipping one would let a builder draw the roofing
tranche on an unfinished foundation.

## LendingPool

| Call | Who | Notes |
|------|-----|-------|
| `set_mortgage_pool` | Admin | Once only |
| `schedule_action` / `execute_action` / `cancel_action` | Admin | Upgrade only |
| `propose_admin` / `accept_admin` | Admin / the proposed address | |
| `deposit` | Any investor | A claim on the pool, issued one-for-one |
| `withdraw` | The investor | Only uncommitted capital |
| `claim_interest` | The investor | Signs for themselves |
| `reserve` / `unreserve` | The wired MortgagePool | Commitment against approved loans |
| `disburse` | The wired MortgagePool | Only from reserved capital |
| `repay` | The wired MortgagePool | Splits principal and interest as the pool instructs |
| `write_off` | The wired MortgagePool | Loss falls on capital |

## MortgagePool

| Call | Who | Notes |
|------|-----|-------|
| `set_underwriter` | Admin | Reversible |
| `schedule_action` / `execute_action` / `cancel_action` | Admin | Upgrade and grace period; a zero grace is refused |
| `propose_admin` / `accept_admin` | Admin / the proposed address | |
| `apply` | The borrower | Verified property, within 80% LTV, one live loan per property |
| `approve` | Registered underwriter | Re-checks the valuation and commits the facility |
| `decline` | Registered underwriter | Only before approval; frees the property |
| `repay` | The borrower | Must cover the instalment or the full payoff |
| `disburse` | **Anyone** | Registry decides; tranche fixed by the loan |
| `mark_default` | **Anyone** | Only past the grace period |

`disburse` and `mark_default` are open on purpose. Neither caller can influence
the outcome: the registry decides whether a stage is releasable and the loan
fixes the tranche, and arrears against the grace period decide a default. Making
them permissionless means a trustee need not wait on the platform once an
inspector has signed, and the platform cannot keep a bad loan off the books by
declining to act.

`disburse` pays the **trustee**, never the borrower, and never the caller. The
point of staged release is that the funds reach the build.

## What the admin cannot do

Worth stating plainly, because it is the question that matters most:

- Move the settlement asset out of any contract
- Mint, transfer or seize a claim on the pool
- Alter a loan's balance, or a borrower's or investor's position
- Release a tranche for a stage the inspector has not signed
- Claim interest on an investor's behalf
- Lend above 80% of valuation, or write a rate above 30%
- Change the wiring between the contracts

Every item on that list would require an upgrade, and every upgrade waits out the
timelock in public first, where `get_scheduled_action` will show it.
