# Authorization

Who may call what, and what each caller can reach. Every entry point is listed;
if a call is not here, it is a getter and open to anyone.

## Roles

| Role | How it is held | How it is granted | How it is revoked |
|------|----------------|-------------------|-------------------|
| Admin | One address per contract, set at deploy | Two-step handover | Handing it on; it cannot be renounced |
| Sponsor | Flag in the registry, per address | `set_sponsor(.., true)` | `set_sponsor(.., false)`, effective on the next call |
| Appraiser | Flag in the registry, per address | `set_appraiser(.., true)` | `set_appraiser(.., false)`, effective on the next call |
| Investor / holder | Any wallet | — | — |
| Offering contract | Address stored once in the registry and the ledger | `set_offering`, once | Only by upgrade |
| IncomeDistributor | Address stored once in the ledger | `set_distributor`, once | Only by upgrade |

A revoked sponsor keeps the properties they already registered — the record
stands — but cannot register or open anything new. Revocation does not claw back
a settled raise.

## PropertyRegistry

| Call | Who | Notes |
|------|-----|-------|
| `set_offering` | Admin | Once only |
| `set_sponsor`, `set_appraiser` | Admin | Reversible |
| `schedule_action`, `execute_action`, `cancel_action` | Admin | Upgrade only, behind the timelock |
| `propose_admin` | Admin | Changes nothing on its own |
| `accept_admin` | The proposed address | Signs for itself |
| `register_property` | Registered sponsor | Starts in `Draft` |
| `open_offering` | The property's **own** sponsor | Requires a published valuation |
| `publish_valuation` | Registered appraiser | **Never** the property's sponsor, even holding both roles |
| `mark_owned`, `mark_failed` | The wired Offering contract | Only from `Offering` status |
| `retire_property` | Admin | Only from `Owned`; one-way |

The sponsor check on `open_offering` is not just "is a sponsor" but "is *this*
property's sponsor". One registered sponsor cannot open another's property.

## ShareLedger

| Call | Who | Notes |
|------|-----|-------|
| `set_offering`, `set_distributor` | Admin | Once only, each |
| `schedule_action`, `execute_action`, `cancel_action` | Admin | Upgrade only |
| `propose_admin` / `accept_admin` | Admin / the proposed address | |
| `issue` | The wired Offering contract | The only way a share comes into existence |
| `transfer` | The holder giving up the shares | Settles both sides' income first |
| `accrue` | The wired IncomeDistributor | Refused when nothing is issued |
| `take_accrued` | The wired IncomeDistributor | Settles and zeroes the position, returns the amount |

`take_accrued` does not itself check the holder's signature — the
IncomeDistributor requires it in `claim` before calling. The ledger accepts the
call because the distributor is wired, and the distributor pays only the address
that signed.

## Offering

| Call | Who | Notes |
|------|-----|-------|
| `schedule_action`, `execute_action`, `cancel_action` | Admin | Upgrade, treasury, fee; fee capped at 10% in code |
| `propose_admin` / `accept_admin` | Admin / the proposed address | |
| `open` | The property's own sponsor | Registry must have it in `Offering` status; one sale per property, ever |
| `subscribe` | Any investor | Before the deadline, up to the total on offer |
| `withdraw_subscription` | The subscriber | Before the deadline only |
| `close` | **Anyone** | Outcome fixed by the sale's state |
| `claim_shares` | Anyone, for a named investor | Pays only that investor; settled sales only |
| `refund` | Anyone, for a named investor | Pays only that investor; failed sales only |

`claim_shares` and `refund` deliberately do not require the investor's
signature. Both deliver strictly to the address named in the call, from that
address's own recorded subscription, so a third party can only ever pay somebody
what they were already owed — useful for a sponsor who wants to settle out a
raise without every investor having to transact.

`close` is permissionless for the same reason it is safe: it reads the sale's
own subscribed total against its soft cap and deadline, and does the one thing
those imply. A sponsor cannot strand subscribers by declining to close a failed
raise.

## IncomeDistributor

| Call | Who | Notes |
|------|-----|-------|
| `schedule_action`, `execute_action`, `cancel_action` | Admin | Upgrade only |
| `propose_admin` / `accept_admin` | Admin / the proposed address | |
| `deposit_income` | **Anyone** | Signs for the funds being paid in |
| `claim` | The holder | Signs for themselves |

`deposit_income` is open on purpose. A deposit only ever gives money away to the
people who already hold shares, so there is nothing to gain by making one and
nothing to protect by restricting it. In practice the sponsor or managing agent
calls it.

## What the admin cannot do

Worth stating plainly, because it is the question that matters most:

- Move the settlement asset out of any contract
- Mint, burn or transfer a share
- Alter a holder's position or their accrued income
- Claim income on someone's behalf
- Raise the protocol fee above 10%
- Change the wiring between contracts

Everything on that list would require an upgrade, and every upgrade waits out
the timelock in public first.
