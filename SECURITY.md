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

- Loss or theft of investor capital in the lending pool
- A tranche released for a stage no inspector signed off, or released twice
- A loan balance that can be inflated, forgiven or made uncomputable
- Interest paid to the wrong party, or that becomes unclaimable
- Capital committed to a build that can be withdrawn out from under it
- Any way to bypass the authorization rules in
  [docs/AUTHORIZATION.md](docs/AUTHORIZATION.md)
- Any way to bypass or shorten the upgrade timelock
- Arithmetic that overflows, truncates in the protocol's favour, or leaves the
  contracts owing more than they hold

**Out of scope:**

- The deployment scripts in `scripts/`, which are operator tooling
- Anything requiring the admin multisig's signers to be compromised — that is a
  stated trust assumption
- Trustee or oracle fraud that the protocol openly does not prevent: see the threat model
- Off-chain systems, including the KYC pipeline, trustee portal and borrower dashboard
- Gas or fee optimisation without a security consequence
- Findings from automated scanners without a concrete exploit path

## Known and accepted risks

These are deliberate, documented in
[docs/THREAT_MODEL.md](docs/THREAT_MODEL.md), and not vulnerabilities:

- **A trustee can take a tranche and not build.** Release in build order limits
  the exposure to one tranche, but the protocol cannot see a building. Recourse
  is legal and off-chain.
- **A corrupt oracle can cause real loss.** Sign-offs are attributed on-chain
  and a trustee cannot sign off their own work, but a dishonest inspection or
  valuation is not detectable on-chain. Oracle vetting and key custody are the
  real control.
- **The admin can upgrade any contract.** Timelocked, cancellable and visible
  while queued, but an upgrade can introduce anything. The timelock is what
  makes it survivable.
- **Foreclosure is off-chain.** A defaulted loan is written off on-chain;
  recovering the property is a matter for local law.

## Supported versions

The protocol is pre-1.0. Only `main` is supported; fixes land there and are
deployed by upgrade. There are no backported releases.
