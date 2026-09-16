# Interest and repayment

What the chain charges, why it is computed this way, and how it differs from the
schedule the borrower dashboard draws.

## The shape of the loan

A Stellar Homes mortgage is a **constant-amortisation** loan:

- Principal amortises in equal monthly slices: `principal / term_months`.
- Interest is charged monthly on the balance **actually drawn** and not yet
  repaid.
- Each instalment is that month's interest plus one slice of principal, so the
  payment falls as the balance does.

It is not a level-payment annuity, which is what most mortgages are and what the
front end projects.

## Why not the annuity

The level-payment formula is `P · r(1 + r)^n / ((1 + r)^n − 1)`. Evaluating it
needs `(1 + r)^n` — a fractional base raised to a power of up to several hundred.
In `no_std` integer arithmetic there are two ways to get it:

1. **Approximate it**, with a truncated series or a lookup table. The borrower
   then has to trust that the approximation is faithful, and the error is not
   something they can check.
2. **Implement fixed-point exponentiation** properly. That is a few hundred
   lines of numerically delicate code, in a contract that moves money, and it
   becomes its own audit surface — larger than the rest of the repayment logic
   put together.

Constant amortisation needs one multiplication and one division per accrual. A
borrower can check any charge on paper. For a protocol whose whole argument is
that money moves only in ways you can verify, that is worth more than matching a
conventional payment profile.

**The contract is the source of truth for what is owed.** Anything rendered
elsewhere — including `buildAmortizationSchedule` in the front end — is a
projection, and where they disagree, the chain is right.

## Accrual

```
divisor   = 10_000 · 12                       // basis points × months in a year
numerator = outstanding · rate_bps · months + carry
interest += numerator / divisor
carry     = numerator % divisor
```

`months` is the number of **whole** 30-day periods since the last accrual, and
`last_accrued_at` advances by exactly that many — never to "now" — so no part
month is ever lost or charged twice, however often accrual runs.

### Why the carry matters

Without it, interest would depend on how often accrual happened to be triggered.

Take a balance of 10,000 at 8.5%, over three months:

| Path | Arithmetic | Charged |
|------|-----------|---------|
| Three separate monthly accruals, no carry | `⌊70.83⌋ × 3` | 210 |
| One three-month accrual, no carry | `⌊212.5⌋` | 212 |

A borrower who called the contract every month would round down twelve times a
year; one who left it alone would round down once. Same debt, different bill.

With the carry, the remainder of each division is kept and added back to the
next numerator, and both paths come to **212**. A test,
`test_interest_does_not_depend_on_how_often_it_is_charged`, drives six monthly
accruals against one six-month accrual and asserts they match exactly.

### A billing month is 30 days

Calendar months would need date arithmetic the contract has no way to do.
A fixed 30-day period means a borrower can work out when the next charge lands
without knowing which month it falls in. The drift against the calendar is
about five days a year, and is disclosed rather than hidden.

## The instalment

```
slice          = principal / term_months
principal_part = min(slice, outstanding)
due            = interest_accrued + principal_part
```

The slice is computed from the **whole facility**, not the drawn balance, so the
loan still clears on schedule even though it is drawn down in stages. The final
instalment is whatever is left.

## Applying a payment

Interest is brought up to date, then:

1. The payment clears accrued interest first.
2. Whatever remains reduces principal.

So a borrower who pays exactly the interest never sees their balance grow, and
one who pays more always reduces what they owe. Anything above the instalment
goes straight to principal and shortens the loan.

A payment must cover **either** the full instalment **or** the whole payoff.
Anything in between is refused, so a part payment cannot quietly stall the
schedule while looking like a payment was made. Overpaying a payoff takes only
what is owed.

## Worked example

A 50,000 facility at 8.5% over 120 months, with the first tranche of 10,000
drawn.

**Month 1.** Interest on 10,000: `10000 × 850 × 1 / 120000 = 70` (carry 100,000
in scaled units). Principal slice: `50000 / 120 = 416`. Instalment: **486**.

After paying it: outstanding 9,584, interest 0, carry preserved.

**Month 2**, with the walls tranche drawn, balance 19,584. Interest:
`19584 × 850 × 1 / 120000` plus the carry from month 1. Principal slice is still
416; the payment is larger than last month's because the balance is.

**Early payoff.** At any point, `payoff_amount` is `outstanding +
interest_accrued`. Paying it closes the loan, returns any undrawn facility to
the pool, and releases the property.

## Default

`next_payment_due` advances by one month with each accepted payment. Once
`now > next_payment_due + grace_secs`, anyone may call `mark_default`:

- The undrawn facility is released back to the pool for someone else's build.
- The drawn balance is written off against investors' capital.
- The property is marked defaulted and freed.

Interest investors have already been credited is untouched — it was earned.

A loan with nothing drawn has no instalment due and cannot default, however long
it sits approved.

## Where the interest goes

`repay` hands the LendingPool the principal and interest separately. Principal
returns to the pool's capital; interest is spread across investors by
shareholding, using the same carry-preserving accumulator, and claimed when they
choose. See [ARCHITECTURE.md](ARCHITECTURE.md) for why the split is made by the
MortgagePool rather than the LendingPool.
