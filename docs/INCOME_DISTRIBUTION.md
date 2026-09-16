# Income distribution

How rent gets from one deposit to thousands of shareholders without the
contract ever looping over them.

## The problem

A property might have ten thousand shareholders. Paying them in a loop is not
possible on-chain: the transaction would exceed every resource limit, and it
would get worse as the property became more widely held. Pushing payments also
means the protocol must know every holder, which is exactly the list that
changes every time anybody trades.

So income is **accrued**, not pushed. A deposit updates one number per property.
Each holder later works out their own share from it, in constant time,
regardless of how many deposits they slept through.

## The accumulator

Each property carries `acc_per_share`: everything ever deposited for it, divided
by the shares issued at the time of each deposit, summed over its life. Because
income per share is a fraction, it is scaled by `SCALE = 10^12` before dividing.

A deposit of `amount` into a property with `issued` shares does:

```
scaled     = amount * SCALE + carry
acc        = acc + scaled / issued
carry      = scaled % issued
```

`carry` is the part that did not divide, kept in the accumulator's own scaled
units. It is added back on the next deposit, so truncation never compounds: the
protocol distributes the whole of what it received, eventually, rather than
shaving a fraction off every month.

## A holder's position

```rust
struct Position {
    balance: i128,      // shares held
    reward_debt: i128,  // what the accumulator was worth against this balance,
                        // at the last settlement
    credited: i128,     // earned, set aside, not yet claimed
}
```

`reward_debt` is bookkeeping, not a liability. It is subtracted so the holder is
paid only for the accumulator's growth *since* they were last settled.

Settling a position:

```
entitled     = balance * acc / SCALE
credited    += entitled - reward_debt
reward_debt  = entitled
```

Re-anchoring after the balance changes:

```
reward_debt = new_balance * acc / SCALE
```

Every balance change — issuance and both sides of a transfer — settles first and
re-anchors after. That pairing is the whole of the correctness argument.

## Worked example

A property issues 1,000 shares: Alice 750, Bob 250. Start with `acc = 0` and
both positions at zero.

**1. Rent of 4,000 arrives.**

```
acc = 0 + 4000 * SCALE / 1000 = 4 * SCALE
```

Alice is owed `750 * 4 = 3,000`. Bob is owed `250 * 4 = 1,000`. Neither position
was touched; both are computed on demand.

**2. Alice sells her whole stake to Bob, without claiming.**

Alice settles first: `entitled = 750 * 4 = 3,000`, so `credited = 3,000`. Her
balance goes to 0 and `reward_debt` re-anchors to `0 * 4 = 0`.

Bob settles: `entitled = 250 * 4 = 1,000`, `credited = 1,000`. His balance goes
to 1,000 and `reward_debt` re-anchors to `1,000 * 4 = 4,000`.

**3. Rent of 4,000 arrives again.**

```
acc = 4 * SCALE + 4000 * SCALE / 1000 = 8 * SCALE
```

Alice: balance 0, so `entitled = 0`, and `credited - reward_debt` leaves her
3,000 — her share of the first month, which selling did not forfeit.

Bob: `entitled = 1,000 * 8 = 8,000`, minus `reward_debt` of 4,000, plus the
1,000 already credited, is **5,000** — all of the second month plus his quarter
of the first. He earns nothing from the first month's other 3,000, because he
did not hold those shares when it arrived.

Total claimed: 3,000 + 5,000 = 8,000, exactly what was deposited.

## Why churning cannot farm it

Passing shares back and forth settles both positions at the same `acc` each
time. Settling twice at an unchanged `acc` adds `entitled - reward_debt = 0`, so
the second and every later round-trip credits nothing. The test
`test_income_cannot_be_farmed_by_churning_shares` asserts exactly this over five
round-trips.

## Why the contract can always pay

The distributor pays out only what `take_accrued` returns, and that is bounded
by `balance * acc / SCALE`, which is bounded by what was deposited — integer
division always rounds down, and the undistributed remainder sits in `carry`.
So the contract's obligations never exceed its holdings. The test
`test_the_contract_always_holds_enough_to_cover_what_it_owes` drives ten
deposits of 7 across three equal holders — a division that never comes out even
— and asserts the balance still covers every claim.

## Precision and bounds

`SCALE` is `10^12` and the registry caps a property at `10^12` shares. The
worst case for the accumulator is a property with a single share taking very
large deposits, where `acc` grows by `amount * 10^12` per deposit. `i128` holds
about `1.7 * 10^38`, leaving room for roughly `10^26` units of the settlement
asset — far beyond any plausible rent roll. `overflow-checks` is left on in the
release profile, so if that bound were ever approached the contract would panic
rather than wrap.

## What is not handled on-chain

- **Tax withholding.** Deposits are net amounts. Whatever is withheld is
  withheld before the money arrives.
- **Distribution schedules.** There is no concept of a month or a quarter. A
  deposit is a deposit; the cadence is the sponsor's business.
- **Expenses.** The protocol splits what it is given. Maintenance, management
  fees and voids are netted off-chain.
