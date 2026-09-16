# Threat model

What the protocol defends against, how, and — just as importantly — what it
does not.

## What is being protected

1. **Subscriptions in escrow.** Money paid into a raise that has not closed.
2. **Proceeds at settlement.** That the right party is paid the right amount.
3. **The cap table.** That shares exist only where they were bought.
4. **Accrued income.** That rent reaches the people who held shares when it
   arrived.

## Trust assumptions

The protocol assumes:

- The Stellar network orders and finalizes transactions correctly.
- The settlement asset's Stellar Asset Contract behaves as a standard token.
- The admin multisig's signers are not all compromised at once.

The protocol does **not** assume sponsors are honest, appraisers are accurate,
investors are friendly, or that anyone will call `close` promptly.

## Threats and mitigations

### A sponsor takes the money and disappears

Not prevented on-chain, and not claimed to be. Subscriptions are escrowed until
the raise settles, so a sponsor cannot touch them mid-raise, but at settlement
the proceeds are theirs. What the protocol provides is procedure, not custody: a
property cannot be sold without a valuation signed by somebody who is not the
sponsor, and that signature is attributed on-chain permanently.

Recourse after settlement is legal and off-chain. This is the protocol's largest
residual risk and operators should treat sponsor vetting as the real control.

### A sponsor inflates the valuation

`publish_valuation` rejects a call from the property's own sponsor, checked
against the property record rather than a role flag — so granting one address
both roles does not create a hole. The valuation is stored with the appraiser's
address and a timestamp.

An appraiser colluding with a sponsor is not prevented. The admin can revoke an
appraiser, but only going forward.

### A sponsor refuses to close a raise that failed

`close` is permissionless. Any address may call it once the sale has sold out or
its deadline has passed, and what it does is computed from the sale's own state.
Subscribers can close the raise themselves and take their refunds.

### An investor subscribes, the raise fails, and refunds run out

Cannot happen. A failed raise never pays the sponsor or the treasury; the escrow
is untouched, so the sum of refunds is exactly the sum of subscriptions.
`test_a_raise_below_its_soft_cap_is_unwound_in_full` asserts the contract's
balance reaches zero and both investors are made whole.

### Double-claiming shares or refunds

Both zero the subscription before transferring or issuing, and a subscription at
zero is removed. `claim_shares` in particular zeroes *before* calling the
ledger, so even a re-entrant ledger would find nothing to claim.

### Farming income by trading shares

Every balance change settles both positions first and re-anchors after. Settling
twice at an unchanged accumulator credits nothing, so round-tripping shares
mints no entitlement. A self-transfer — which would settle the same position
twice against itself — is rejected outright.

### Buying in just before a deposit to capture past income

The accumulator moves only when income arrives. A buyer's `reward_debt` is
anchored to the accumulator's value at purchase, so they earn from that point
forward only. Buying the day before a deposit earns all of that deposit, which
is correct: they held the shares when it arrived.

### Rent deposited before anyone holds shares

Refused. `accrue` rejects a property with nothing issued, and the distributor
accrues before transferring, so the money never moves. Allowing it would have
handed the whole amount to whoever claimed their shares first.

### Dust accumulating until the contract is insolvent

Truncation from integer division is retained in a per-property `carry`, in the
accumulator's own scaled units, and folded into the next deposit. The
contract's obligations are therefore always at or below what it holds, never
above.

### A malicious contract impersonating the Offering or the distributor

Each privileged caller is an address stored once at setup and compared on every
call. Those addresses cannot be changed after they are set — not by the admin,
not by anyone — so impersonation requires an upgrade, which waits out the
timelock in public.

### A rogue admin

The admin cannot move the settlement asset, mint or transfer shares, alter a
position, or claim on anyone's behalf. No such entry point exists. What the
admin *can* do:

| Power | Constraint |
|-------|-----------|
| Upgrade any contract | Timelocked, cancellable, visible while queued |
| Change the treasury | Timelocked; affects future settlements only |
| Change the fee | Timelocked, and capped at 10% in code |
| Register or revoke sponsors and appraisers | Forward-looking only |
| Retire an owned property | Does not touch shares or unclaimed income |

An upgrade is unrestricted in what it could introduce — that is the nature of
upgradeable code. The timelock is what makes it survivable: shareholders can see
a queued upgrade and claim their income before it executes. Deploy with a
timelock long enough for that to be a real option.

### Admin key loss

`accept_admin` requires the proposed address to sign for itself, so a typo
cannot transfer the role to an address nobody controls. The role cannot be
renounced, so a contract always has an admin.

If the admin key is lost outright, the contracts keep working — every user-facing
path is unaffected — but can never be upgraded or have roles changed again.

### Front-running

Subscriptions are first-come, and a raise can be filled by whoever transacts
first. This is visible and inherent; the soft cap and the deadline are the
protections an investor has, not ordering.

Nothing else in the protocol is order-sensitive in a way that can be exploited:
income accrual is path-independent, and `close` produces the same outcome
whoever calls it.

## Out of scope

- Whether the building exists, or its title is clean
- Whether deposited rent matches rent collected
- Off-chain identity, KYC and accreditation
- Tax
- The value of a share on any secondary venue
