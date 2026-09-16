# Storage and TTL policy

Soroban charges rent on stored state and archives entries that let their
lifetime lapse. This is how each contract keeps what matters alive without
paying for what does not.

## The constants

Identical in all four contracts:

```rust
const DAY_IN_LEDGERS: u32 = 17_280;          // at 5s ledger close
const EXTEND_TO: u32      = 120 * DAY_IN_LEDGERS;  // ~120 days
const THRESHOLD: u32      = 90 * DAY_IN_LEDGERS;   // ~90 days
```

An entry is extended back up to about 120 days whenever its remaining lifetime
falls below about 90. Both are well inside the network's maximum entry lifetime
of roughly 180 days, so an extension is never rejected for overshooting.

The gap between the two is deliberate. Extending on every single call would pay
rent constantly; extending only at the last moment would risk archival between
uses. Ninety days of slack means a record touched even once a quarter never
lapses.

## Instance storage

Configuration and wiring: the admin, the pending admin, the timelock, the
scheduled action, and every wired contract address.

Instance storage shares its lifetime with the contract's code. Extending it
extends both, so the contract and the configuration it needs can never expire
apart. Every entry point calls `extend_instance` first, including the
constructor — so a contract that is used at all stays live.

| Contract | In instance storage |
|----------|--------------------|
| PropertyRegistry | Admin, PendingAdmin, TimelockSecs, Scheduled, MortgagePool, NextId |
| LendingPool | Admin, PendingAdmin, TimelockSecs, Scheduled, SettlementToken, MortgagePool, and every running total (TotalCapital, TotalReserved, TotalLent, TotalInterest, TotalWrittenOff, TotalShares, Acc, Carry) |
| MortgagePool | Admin, PendingAdmin, TimelockSecs, Scheduled, Registry, LendingPool, GraceSecs, NextId |

`NextId` lives in instance storage on purpose: it must never be archived, or a
reset counter would let a new property reuse a closed one's id and inherit its
loan history.

The LendingPool's running totals live there too, rather than in persistent
storage, because they are read and written on every single call. Keeping them
with the instance means they are renewed by the same `extend_instance` that
keeps the contract alive, and a pool holding anybody's capital is being called
often enough that it never lapses.

## Persistent storage

Everything that belongs to a particular property or person.

| Contract | Key | Holds |
|----------|-----|-------|
| PropertyRegistry | `Property(u64)` | The property record |
| PropertyRegistry | `Milestone(u64, u32)` | One construction stage |
| PropertyRegistry | `Trustee(Address)`, `Oracle(Address)` | Role flags |
| LendingPool | `Position(Address)` | An investor's shares, reward debt and credited interest |
| MortgagePool | `Mortgage(u64)` | The loan record |
| MortgagePool | `PropertyMortgage(u64)` | The live loan against a property |
| MortgagePool | `Underwriter(Address)` | Role flag |

Persistent entries are extended on **every read and every write**, so a record
in use renews itself. Reads go through one accessor per contract that extends
only when the entry actually exists, so reading a holder who has never held
anything does not create an entry.

Writes go through one `save` helper that sets and extends together. There is no
path that writes a persistent entry without extending it.

## Deliberate removals

`PropertyMortgage` is **removed** when a loan is declined, paid off or
defaulted, not left pointing at a closed loan. It exists to answer one question
— is there a live mortgage against this property — and once the answer is no,
the entry has nothing to say. Removing it is also what frees the property to be
financed again.

Role flags are removed on revocation for the same reason: absent and false are
the same answer, and the absent one costs nothing to keep.

## What archival would mean

Nothing is lost permanently: an archived Soroban entry can be restored by anyone
willing to pay the restoration fee, and it comes back with its contents intact.
The practical consequence of letting a `Position` lapse is that its holder must
restore it before claiming, not that their shares are gone.

The entries most at risk are an investor's `Position` where they deposit once and
never transact again, and a `Mortgage` on a loan nobody touches for four months —
though a borrower who pays monthly renews their own record every time.

Read-only getters that a dashboard polls (`claimable_interest`, `pool_state`,
`current_balance`) deliberately do **not** extend persistent entries: a
dashboard refresh should not quietly spend somebody's rent. Every path that
actually changes state does extend.

## Rules for new code

1. Configuration, wiring and protocol-wide running totals go in instance
   storage; anything keyed by a property, loan or person goes in persistent.
2. Every public entry point calls `extend_instance` first.
3. Persistent reads extend only when the entry exists.
4. Persistent writes extend in the same helper that sets.
5. A value that has become meaningless is removed, not stored as zero.
