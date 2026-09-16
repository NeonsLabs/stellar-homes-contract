# Events

Every event the contracts publish, for indexers and dashboards. Topics are
`(contract_symbol, event_symbol)`; the data column is the tuple published
alongside. Symbols are abbreviated where Soroban's nine-character
`symbol_short!` limit requires it.

## PropertyRegistry — topic prefix `registry`

| Event | Data | When |
|-------|------|------|
| `submitted` | `(property_id: u64, trustee: Address)` | A trustee registers a property |
| `trustee` | `(trustee: Address, authorized: bool)` | A trustee is granted or revoked |
| `oracle` | `(oracle: Address, authorized: bool)` | An oracle is granted or revoked |
| `title` | `(property_id: u64, oracle: Address)` | A title passes the land-registry check |
| `valuation` | `(property_id: u64, oracle: Address, usdc_value: i128)` | A surveyor publishes a valuation |
| `evidence` | `(property_id: u64, stage: u32, evidence_hash: BytesN<32>)` | Build evidence is submitted |
| `verified` | `(property_id: u64, stage: u32, oracle: Address)` | A stage passes inspection |
| `released` | `(property_id: u64, stage: u32)` | A stage's tranche has been paid |
| `status` | `(property_id: u64, status: PropertyStatus)` | A property changes status |

`valuation` fires on every republication, so the history of a property's
appraisals is reconstructible. `status` covers Verified, Mortgaged, Repaid and
Defaulted.

## LendingPool — topic prefix `pool`

| Event | Data | When |
|-------|------|------|
| `deposit` | `(investor: Address, amount: i128, total_capital: i128)` | Capital comes in |
| `withdraw` | `(investor: Address, amount: i128, total_capital: i128)` | Capital goes out |
| `interest` | `(investor: Address, amount: i128)` | An investor collects yield |
| `reserve` | `(amount: i128, total_reserved: i128)` | A facility is committed |
| `unreserve` | `(amount: i128, total_reserved: i128)` | A commitment is released |
| `disburse` | `(to: Address, amount: i128, total_lent: i128)` | A tranche is paid out |
| `repay` | `(from: Address, principal: i128, interest: i128)` | A repayment is banked |
| `writeoff` | `(principal: i128, total_written_off: i128)` | Principal is written off |

Each carries the running total it affects, so an indexer can track the pool
without replaying every event from genesis.

## MortgagePool — topic prefix `mortgage`

| Event | Data | When |
|-------|------|------|
| `applied` | `(id: u64, property_id: u64, borrower: Address, principal: i128)` | An application is filed |
| `approved` | `(id: u64, underwriter: Address, principal: i128)` | Underwriting commits the facility |
| `declined` | `(id: u64, underwriter: Address)` | An application is refused |
| `disbursd` | `(id: u64, stage: u32, tranche: i128, trustee: Address)` | A tranche is released |
| `repaid` | `(id: u64, amount: i128, principal: i128, interest: i128)` | A payment is applied |
| `default` | `(id: u64, written_off: i128, undrawn: i128)` | A loan is written off |
| `undrwrtr` | `(underwriter: Address, authorized: bool)` | An underwriter is granted or revoked |

`repaid` gives the split as applied, so an indexer never has to re-derive it:
`amount` is what the borrower paid, and `principal + interest` equals it.

## Reconstructing state from events

A loan's balance is its disbursements minus the principal component of its
repayments. The pool's capital is the last `deposit`/`withdraw`/`repay` total.
A property's build progress is the set of `verified` and `released` stages.

State can always be read directly instead — `get_mortgage`, `current_balance`,
`payoff_amount`, `amount_due`, `pool_state`, `get_milestone` and
`claimable_interest` are all open getters. Events exist for history and
notification, not because state is unreadable.

Note that `current_balance`, `amount_due` and `payoff_amount` compute accrual
without writing it, so they are accurate the instant they are called even if no
transaction has touched the loan for months.
