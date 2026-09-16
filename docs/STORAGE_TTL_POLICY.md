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
| PropertyRegistry | Admin, PendingAdmin, TimelockSecs, Scheduled, Offering, NextId |
| ShareLedger | Admin, PendingAdmin, TimelockSecs, Scheduled, Offering, Distributor |
| Offering | Admin, PendingAdmin, TimelockSecs, Scheduled, Registry, ShareLedger, SettlementToken, Treasury, FeeBps |
| IncomeDistributor | Admin, PendingAdmin, TimelockSecs, Scheduled, ShareLedger, SettlementToken |

`NextId` lives in instance storage on purpose: it must never be archived, or a
reset counter would let a new property reuse a retired property's id and inherit
its cap table.

## Persistent storage

Everything that belongs to a particular property or person.

| Contract | Key | Holds |
|----------|-----|-------|
| PropertyRegistry | `Property(u64)` | The property record |
| PropertyRegistry | `Sponsor(Address)`, `Appraiser(Address)` | Role flags |
| ShareLedger | `Position(u64, Address)` | Balance, reward debt, credited income |
| ShareLedger | `Issued(u64)`, `Acc(u64)`, `Carry(u64)` | Per-property totals |
| Offering | `Sale(u64)` | The sale record |
| Offering | `Subscription(u64, Address)` | One investor's subscription |
| IncomeDistributor | `Deposited(u64)`, `Claimed(u64)` | Per-property running totals |

Persistent entries are extended on **every read and every write**, so a record
in use renews itself. Reads go through one accessor per contract that extends
only when the entry actually exists, so reading a holder who has never held
anything does not create an entry.

Writes go through one `save` helper that sets and extends together. There is no
path that writes a persistent entry without extending it.

## Deliberate removals

A subscription that reaches zero — claimed, refunded, or fully withdrawn — is
**removed** rather than stored as zero. There is nothing left to say about it,
and a spent investor should not keep paying rent on an entry that says nothing.
Reading a removed subscription returns 0, which is the same answer.

Role flags are removed on revocation for the same reason.

## What archival would mean

Nothing is lost permanently: an archived Soroban entry can be restored by anyone
willing to pay the restoration fee, and it comes back with its contents intact.
The practical consequence of letting a `Position` lapse is that its holder must
restore it before claiming, not that their shares are gone.

The entries most at risk are those belonging to a holder who buys once and never
transacts again for four months. This is why `accrued_of` — a read-only getter a
dashboard calls routinely — does **not** extend anything, while every path that
touches a position does. A holder who claims even once a quarter is never
troubled.

## Rules for new code

1. Configuration and wiring go in instance storage; everything else persistent.
2. Every public entry point calls `extend_instance` first.
3. Persistent reads extend only when the entry exists.
4. Persistent writes extend in the same helper that sets.
5. A value that has become meaningless is removed, not stored as zero.
