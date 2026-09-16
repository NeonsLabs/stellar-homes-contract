# Deployment

## Prerequisites

- Rust stable, with the `wasm32v1-none` target: `rustup target add wasm32v1-none`
- Stellar CLI: `cargo install --locked stellar-cli`
- A funded deploying key: `stellar keys generate sh-admin --network testnet`
- The contract id of the settlement asset's Stellar Asset Contract

## Build

```bash
make build
```

Use `wasm32v1-none`, not `wasm32-unknown-unknown`. Recent Rust enables wasm
features on the latter (`reference-types`) that the Soroban VM rejects, and the
failure only shows up at deploy time — after you have paid for the upload.

Verify the four artifacts exist before going further:

```
contracts/target/wasm32v1-none/release/sh_property_registry.wasm
contracts/target/wasm32v1-none/release/sh_share_ledger.wasm
contracts/target/wasm32v1-none/release/sh_offering.wasm
contracts/target/wasm32v1-none/release/sh_income_distributor.wasm
```

## Deploy

```bash
export USDC=<settlement asset contract id>
export SPONSOR=<first sponsor's address>
export APPRAISER=<appraiser's address>
export TIMELOCK_SECS=172800     # 48 hours
export FEE_BPS=250              # 2.5%

./scripts/deploy.sh
```

The script prints `REGISTRY=`, `LEDGER=`, `OFFERING=` and `INCOME=`. Save them.

`SPONSOR` and `APPRAISER` must differ — the registry will not let a sponsor
value their own property, and the script refuses the configuration up front
rather than leaving you with a property that can never be offered.

### Why the script must run to completion

Three wiring calls can be made only once:

```
registry.set_offering(offering)
ledger.set_offering(offering)
ledger.set_distributor(distributor)
```

They are one-shot because rewiring later would let an admin point the ledger at
a contract that mints itself a majority of any property. The cost of that
safety is that a deployment interrupted between the deploys and the wiring
cannot be repaired — you would have to redeploy from scratch. Do not run the
script against a flaky connection.

Verify the wiring took before doing anything else:

```bash
stellar contract invoke --id $REGISTRY --network testnet -- get_offering
stellar contract invoke --id $LEDGER   --network testnet -- get_offering
stellar contract invoke --id $LEDGER   --network testnet -- get_distributor
```

## Choosing the timelock

The timelock is fixed at deployment and cannot be changed without an upgrade —
which is itself timelocked. It is the window in which shareholders can see a
queued upgrade and act before it executes.

| Network | Suggested | Reasoning |
|---------|-----------|-----------|
| Local | 0 | Nothing to protect; makes tests fast |
| Testnet | 48 hours | Long enough to exercise the flow realistically |
| Mainnet | 7 days | Long enough that a holder checking weekly sees a queued upgrade and can claim their income first |

Too short and the timelock is theatre. Too long and a genuine security fix waits
while the bug is public.

## Hand the admin role to a multisig

The deploying key is a single point of failure. Move the role to a multisig
before the protocol holds anything.

```bash
export MULTISIG=<multisig account address>
./scripts/handover-to-multisig.sh
```

This proposes on all four contracts. Nothing changes until the multisig accepts
from its own account:

```bash
for id in $REGISTRY $LEDGER $OFFERING $INCOME; do
  stellar contract invoke --id $id --source-account <multisig> \
    --network testnet -- accept_admin --new_admin $MULTISIG
done
```

Confirm with `get_admin` on each of the four before retiring the old key.

## Smoke test

```bash
./scripts/smoke-testnet.sh
```

Registers a property, appraises it, sells it out, settles, pays a month of rent
in and claims it. Every step is a real transaction; the script refuses to run
against mainnet.

## Registering more sponsors and appraisers

These stay changeable for the protocol's life:

```bash
stellar contract invoke --id $REGISTRY --source-account <admin> --network testnet \
  -- set_sponsor --admin <admin address> --sponsor <address> --authorized true
```

Revocation is forward-looking. A revoked sponsor keeps the properties they
already registered and any settled raise stands; they simply cannot register or
open anything new.

## Upgrading

Every upgrade is a three-step sequence with the timelock in the middle.

**1. Build and upload the new code.**

```bash
make build
stellar contract upload --wasm contracts/target/wasm32v1-none/release/sh_share_ledger.wasm \
  --source-account <admin> --network testnet
```

Note the wasm hash it prints.

**2. Schedule it.**

```bash
stellar contract invoke --id $LEDGER --source-account <admin> --network testnet \
  -- schedule_action --admin <admin address> \
  --action '{"Upgrade":"<wasm hash>"}'
```

Only one action may be pending per contract, so a queued upgrade is visible to
anyone calling `get_scheduled_action`. Announce it.

**3. Execute, after the timelock.**

```bash
stellar contract invoke --id $LEDGER --source-account <admin> --network testnet \
  -- execute_action --admin <admin address>
```

`cancel_action` withdraws it at any point before that.

### Before scheduling an upgrade that changes stored types

The contract keeps its address and its storage across an upgrade; only the code
is replaced. Changing the layout of a stored type — `Position`, `Property`,
`Sale` — without a migration will make existing entries unreadable, and for
`Position` that means a holder's shares and accrued income.

If a stored type changes, the upgrade must either read the old layout and write
the new one, or the new code must tolerate both. Flag it on the PR: the template
has a checkbox for exactly this.

## Mainnet

`deploy.sh` refuses mainnet without `CONFIRM_MAINNET=yes`. Before setting it:

- [ ] The full suite passes, and coverage has not dropped
- [ ] The smoke test has run clean on testnet against this exact build
- [ ] The timelock is set to a mainnet-appropriate value
- [ ] The multisig exists, is funded, and its signers have tested `accept_admin`
- [ ] The treasury address is correct and controlled
- [ ] `FEE_BPS` is the intended fee
- [ ] Sponsors and appraisers are distinct addresses, each vetted
- [ ] Someone other than the deployer has read the wiring output
