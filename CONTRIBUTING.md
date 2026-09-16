# Contributing

## Getting set up

```bash
rustup target add wasm32v1-none
cargo install --locked stellar-cli
make all
```

`make all` runs exactly what CI runs: formatting, clippy, the test suites and
the wasm build. If it passes locally, CI should agree.

## Before you open a PR

```bash
make fmt        # apply formatting
make all        # fmt-check, clippy, test, build
```

Clippy runs with `-D warnings`. A warning is a failure.

## Writing the code

The four contracts share one shape deliberately. New code should look like the
code next to it.

- **`#![no_std]` throughout.** No `std`, no allocation outside the SDK's types.
- **Errors are `#[contracterror]` variants**, raised with `panic_with_error!`.
  Never `unwrap()` on something a caller controls; use
  `unwrap_or_else(|| panic_with_error!(..))` with the error that explains it.
- **Every public entry point calls `extend_instance` first.**
- **Authorization is explicit.** `require_auth()` on the acting address, then a
  check that the address is the one allowed. Both, in that order.
- **Amounts are `i128` and validated positive** before anything else happens.
- **State changes before external calls.** Zero a subscription before issuing
  against it; the ordering is what makes re-entrancy a non-issue.

### Comments

Comment the *why*, not the what. `// set the admin` above a line that sets the
admin is noise. Explain the reasoning a reader could not recover from the code:
why a value is set once, why a check exists, what would go wrong without it.

Doc comments on public functions should say what the function is for and what
constrains it, in prose. Look at `ShareLedger::accrue` or
`Offering::close` for the register.

## Tests

Every crate has its tests in `src/test.rs`, behind `#[cfg(test)]`.

- **Name tests for the behaviour they pin down**, not the function they call.
  `test_interest_does_not_depend_on_how_often_it_is_charged`, not
  `test_accrue_2`.
- **Test the ways it should fail**, with `try_*` and `.is_err()`. A test that
  only covers the happy path has covered the easy half.
- **Assert on money.** Where the settlement asset moves, assert the balances of
  everyone involved, including the contract's own.
- **Use the `Setup` struct pattern** already in each suite rather than building
  an environment inline.

Coverage is enforced at 80% of lines in CI.

## Commits

Conventional commits, scoped to the crate:

```
feat(ledger): settle both sides of a transfer before moving shares
fix(offering): reject a withdrawal after the deadline
docs(threat-model): explain what a rogue admin cannot reach
```

The subject line says what changed. The body says why it needed to, and what
would go wrong without it. Commit messages are the only place some of this
reasoning survives — write them for whoever is reading the blame in a year.

## Changes that need extra care

Flag these on the PR; the template has checkboxes for them.

- **Anything that moves the settlement asset.** Say which balances change and
  who can trigger it.
- **Anything that changes who may call what.** Update
  [docs/AUTHORIZATION.md](docs/AUTHORIZATION.md) in the same PR.
- **Anything that changes a stored type.** `Property`, `Milestone`, `Mortgage`
  and `Position` are persisted; changing their layout breaks existing entries on
  a deployed contract unless the upgrade migrates them. See
  [docs/DEPLOYMENT.md](docs/DEPLOYMENT.md#before-scheduling-an-upgrade-that-changes-stored-types).
- **Anything that grows the wasm.** CI fails on more than 5% growth over the
  recorded baseline. If the growth is intended, update
  `.github/wasm-size-baseline.txt` in the same PR and say why in the body.

## Security

Do not open an issue for a vulnerability. See [SECURITY.md](SECURITY.md).
