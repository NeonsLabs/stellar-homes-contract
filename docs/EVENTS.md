# Events

Every event the contracts publish, for indexers and dashboards. Topics are given
as `(contract_symbol, event_symbol)`; the data column is the tuple published
alongside.

## PropertyRegistry — topic prefix `registry`

| Event | Data | When |
|-------|------|------|
| `register` | `(property_id: u64, sponsor: Address)` | A sponsor records a new property |
| `sponsor` | `(sponsor: Address, authorized: bool)` | A sponsor is granted or revoked |
| `appraisr` | `(appraiser: Address, authorized: bool)` | An appraiser is granted or revoked |
| `valuation` | `(property_id: u64, appraiser: Address, valuation: i128)` | A valuation is published |
| `status` | `(property_id: u64, status: PropertyStatus)` | A property changes status |

`status` covers every transition: to `Offering` when the sponsor opens it, to
`Owned` or `Failed` when the Offering contract closes the sale, and to `Retired`
when the admin winds it up.

## ShareLedger — topic prefix `ledger`

| Event | Data | When |
|-------|------|------|
| `issue` | `(property: u64, to: Address, shares: i128, total_issued: i128)` | Shares are created for an investor |
| `transfer` | `(property: u64, from: Address, to: Address, shares: i128)` | Shares change hands |
| `accrue` | `(property: u64, amount: i128, issued: i128)` | Income is spread across a property's shares |
| `settled` | `(property: u64, holder: Address, owed: i128)` | A holder's entitlement is drawn down |

`issue` carries the running `total_issued` so an indexer can track a property's
outstanding supply without replaying every event.

`settled` fires only when the amount is non-zero, and always immediately
precedes the matching `income`/`claim` payout in the same transaction.

## Offering — topic prefix `offering`

| Event | Data | When |
|-------|------|------|
| `open` | `(property: u64, sponsor: Address, min_shares: i128, deadline: u64)` | A sale opens |
| `subscrib` | `(property: u64, investor: Address, shares: i128, cost: i128)` | An investor subscribes |
| `unsubscr` | `(property: u64, investor: Address, shares: i128, refund: i128)` | An investor withdraws before the deadline |
| `claimed` | `(property: u64, investor: Address, shares: i128)` | An investor takes delivery after settlement |
| `refund` | `(property: u64, investor: Address, amount: i128)` | An investor is refunded after a failure |
| `close` | `(property: u64, status: SaleStatus, subscribed: i128)` | A sale settles or is unwound |

Event symbols are abbreviated to fit Soroban's nine-character `symbol_short!`
limit: `subscrib` for subscribe, `unsubscr` for withdrawal.

## IncomeDistributor — topic prefix `income`

| Event | Data | When |
|-------|------|------|
| `deposit` | `(property: u64, from: Address, amount: i128, total: i128)` | Rent is paid in |
| `claim` | `(property: u64, holder: Address, owed: i128)` | A holder collects |

Both carry enough to reconcile without a state read: `deposit` includes the
running total ever deposited for the property.

## Reconstructing state from events

A property's outstanding supply is the latest `total_issued` from `ledger/issue`.
A holder's balance is their `claimed` shares plus incoming `transfer`s minus
outgoing ones. Unclaimed income for a property is the last `deposit` total minus
the sum of `income/claim` amounts.

Balances and accrued income can always be read directly instead —
`balance_of`, `accrued_of`, `total_issued` and `unclaimed` are all open getters.
Events exist for history and notification, not because state is unreadable.
