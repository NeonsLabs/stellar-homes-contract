# Security

## Reporting a vulnerability

**Do not open a public issue.**

Report privately through GitHub's advisory form on this repository
(Security → Report a vulnerability), which creates a private thread with the
maintainers.

Please include:

- Which contract is affected, and the commit or deployed contract id
- What an attacker can do, concretely — who loses what
- The steps to reproduce it; a failing test is the most useful form
- Anything you know about how widely it applies

We will acknowledge within 72 hours and keep you updated as we work on it. If
you would like credit in the advisory, say so and tell us how to name you.

## Scope

**In scope** — everything under `contracts/`:

- Loss or theft of escrowed subscriptions
- Loss or theft of deposited rental income
- Shares issued where they were not bought, or a cap table that can be corrupted
- Income paid to the wrong party, or income that becomes unclaimable
- Any way to bypass the authorization rules in
  [docs/AUTHORIZATION.md](docs/AUTHORIZATION.md)
- Any way to bypass or shorten the upgrade timelock
- Arithmetic that overflows, truncates in the protocol's favour, or leaves the
  contracts owing more than they hold

**Out of scope:**

- The deployment scripts in `scripts/`, which are operator tooling
- Anything requiring the admin multisig's signers to be compromised — that is a
  stated trust assumption
- Sponsor fraud that the protocol openly does not prevent: see the threat model
- Off-chain systems, including the sponsor portal and investor dashboard
- Gas or fee optimisation without a security consequence
- Findings from automated scanners without a concrete exploit path

## Known and accepted risks

These are deliberate, documented in
[docs/THREAT_MODEL.md](docs/THREAT_MODEL.md), and not vulnerabilities:

- **A sponsor can abscond with settled proceeds.** The protocol escrows
  subscriptions until a raise settles, but at settlement the money is the
  sponsor's. Recourse is legal and off-chain. Sponsor vetting is the real
  control.
- **An appraiser can collude with a sponsor.** Valuations are attributed
  on-chain and a sponsor cannot value their own property, but an inaccurate
  independent valuation is not detectable on-chain.
- **The admin can upgrade any contract.** Timelocked, cancellable and visible
  while queued, but an upgrade can introduce anything. The timelock is what
  makes it survivable.
- **Rent must be deposited honestly.** The protocol splits what it is given. It
  cannot know what was collected.

## Supported versions

The protocol is pre-1.0. Only `main` is supported; fixes land there and are
deployed by upgrade. There are no backported releases.
