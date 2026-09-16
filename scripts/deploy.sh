#!/usr/bin/env bash
# Deploy the three Stellar Homes contracts and wire them together.
#
# Required environment:
#   USDC       contract id of the settlement asset's Stellar Asset Contract
#   TRUSTEE    address of the first trustee to authorize
#   ORACLE     address of the land-registry / surveyor / inspection oracle
#              — must differ from TRUSTEE
#
# Optional environment:
#   NETWORK        stellar network name                    (default: testnet)
#   ADMIN          `stellar keys` identity that deploys the contracts and is
#                  their first admin; hand the role to a multisig afterwards
#                  with scripts/handover-to-multisig.sh    (default: sh-admin)
#   UNDERWRITER    address allowed to approve applications (default: the admin)
#   GRACE_SECS     how long arrears are tolerated before anyone may default a
#                  loan                                    (default: 14 days)
#   TIMELOCK_SECS  delay before a scheduled upgrade or grace change can run
#                                                          (default: 48 hours)
#
# Prints REGISTRY=, LENDING= and MORTGAGE= lines that other scripts can source.
set -euo pipefail

NETWORK=${NETWORK:-testnet}
ADMIN=${ADMIN:-sh-admin}
: "${USDC:?set USDC to the settlement asset contract id}"
: "${TRUSTEE:?set TRUSTEE to the first trustee's address}"
: "${ORACLE:?set ORACLE to the verification oracle's address}"
GRACE_SECS=${GRACE_SECS:-1209600}
TIMELOCK_SECS=${TIMELOCK_SECS:-172800}

if [ "$NETWORK" = "mainnet" ] && [ "${CONFIRM_MAINNET:-}" != "yes" ]; then
  echo "Refusing to deploy to mainnet without CONFIRM_MAINNET=yes." >&2
  exit 1
fi

# The registry refuses a valuation or an inspection signed by the property's own
# trustee, so this configuration would leave every property unlendable. Fail
# here rather than after four deploys.
if [ "$TRUSTEE" = "$ORACLE" ]; then
  echo "TRUSTEE and ORACLE must differ: a trustee cannot verify their own property." >&2
  exit 1
fi

cd "$(dirname "$0")/.."
ADMIN_ADDR=$(stellar keys address "$ADMIN")
UNDERWRITER=${UNDERWRITER:-$ADMIN_ADDR}
WASM=contracts/target/wasm32v1-none/release

# wasm32v1-none, not wasm32-unknown-unknown: recent Rust enables wasm features
# on the latter (reference-types) that the Soroban VM rejects at deploy time.
cargo build --manifest-path contracts/Cargo.toml --target wasm32v1-none --release

deploy() {
  local name=$1; shift
  stellar contract deploy --wasm "$WASM/$name.wasm" \
    --source-account "$ADMIN" --network "$NETWORK" -- "$@"
}
invoke() {
  local id=$1; shift
  stellar contract invoke --id "$id" \
    --source-account "$ADMIN" --network "$NETWORK" -- "$@" >/dev/null
}

# Each constructor runs inside its deploy transaction, so there is no window in
# which an uninitialized contract could be claimed by someone else.
REGISTRY=$(deploy sh_property_registry \
  --admin "$ADMIN_ADDR" --timelock_secs "$TIMELOCK_SECS")
LENDING=$(deploy sh_lending_pool \
  --admin "$ADMIN_ADDR" --settlement_token "$USDC" --timelock_secs "$TIMELOCK_SECS")
MORTGAGE=$(deploy sh_mortgage_pool \
  --admin "$ADMIN_ADDR" --registry "$REGISTRY" --lending_pool "$LENDING" \
  --grace_secs "$GRACE_SECS" --timelock_secs "$TIMELOCK_SECS")

# Wiring that needs an address which did not exist at construction time. Both
# can be set only once, so run the whole script rather than stopping midway: a
# half-wired deployment cannot be repaired without an upgrade.
invoke "$REGISTRY" set_mortgage_pool --admin "$ADMIN_ADDR" --pool "$MORTGAGE"
invoke "$LENDING"  set_mortgage_pool --admin "$ADMIN_ADDR" --pool "$MORTGAGE"

# Roles. These stay changeable for the protocol's life.
invoke "$REGISTRY" set_trustee     --admin "$ADMIN_ADDR" --trustee "$TRUSTEE" --authorized true
invoke "$REGISTRY" set_oracle      --admin "$ADMIN_ADDR" --oracle "$ORACLE" --authorized true
invoke "$MORTGAGE" set_underwriter --admin "$ADMIN_ADDR" --underwriter "$UNDERWRITER" --authorized true

cat <<OUT
REGISTRY=$REGISTRY
LENDING=$LENDING
MORTGAGE=$MORTGAGE
OUT
