#!/usr/bin/env bash
# Deploy the four Stellar Homes contracts and wire them together.
#
# Required environment:
#   USDC       contract id of the settlement asset's Stellar Asset Contract
#   SPONSOR    address of the first property sponsor to authorize
#   APPRAISER  address allowed to publish valuations; never also a sponsor
#
# Optional environment:
#   NETWORK        stellar network name                    (default: testnet)
#   ADMIN          `stellar keys` identity that deploys the contracts and is
#                  their first admin; hand the role to a multisig afterwards
#                  with scripts/handover-to-multisig.sh    (default: sh-admin)
#   TREASURY       where the protocol fee is sent          (default: the admin)
#   FEE_BPS        protocol fee on a settled raise, in basis points; the
#                  contract refuses anything above 1000    (default: 250)
#   TIMELOCK_SECS  delay before a scheduled upgrade, treasury or fee change
#                  can run                                 (default: 48 hours)
#
# Prints REGISTRY=, LEDGER=, OFFERING= and INCOME= lines that other scripts
# can source.
set -euo pipefail

NETWORK=${NETWORK:-testnet}
ADMIN=${ADMIN:-sh-admin}
: "${USDC:?set USDC to the settlement asset contract id}"
: "${SPONSOR:?set SPONSOR to the first property sponsor's address}"
: "${APPRAISER:?set APPRAISER to the valuation appraiser's address}"
FEE_BPS=${FEE_BPS:-250}
TIMELOCK_SECS=${TIMELOCK_SECS:-172800}

if [ "$NETWORK" = "mainnet" ] && [ "${CONFIRM_MAINNET:-}" != "yes" ]; then
  echo "Refusing to deploy to mainnet without CONFIRM_MAINNET=yes." >&2
  exit 1
fi

cd "$(dirname "$0")/.."
ADMIN_ADDR=$(stellar keys address "$ADMIN")
TREASURY=${TREASURY:-$ADMIN_ADDR}
WASM=contracts/target/wasm32v1-none/release

if [ "$SPONSOR" = "$APPRAISER" ]; then
  echo "SPONSOR and APPRAISER must differ: a sponsor may not value their own property." >&2
  exit 1
fi

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
LEDGER=$(deploy sh_share_ledger \
  --admin "$ADMIN_ADDR" --timelock_secs "$TIMELOCK_SECS")
OFFERING_CONFIG="{\"settlement_token\":\"$USDC\",\"treasury\":\"$TREASURY\",\"fee_bps\":$FEE_BPS}"
OFFERING=$(deploy sh_offering \
  --admin "$ADMIN_ADDR" --registry "$REGISTRY" --share_ledger "$LEDGER" \
  --config "$OFFERING_CONFIG" --timelock_secs "$TIMELOCK_SECS")
INCOME=$(deploy sh_income_distributor \
  --admin "$ADMIN_ADDR" --share_ledger "$LEDGER" --settlement_token "$USDC" \
  --timelock_secs "$TIMELOCK_SECS")

# Wiring that needs addresses which did not exist at construction time. Each of
# these can be set only once, so run the whole script rather than stopping
# midway: a half-wired deployment cannot be repaired without an upgrade.
invoke "$REGISTRY" set_offering    --admin "$ADMIN_ADDR" --offering "$OFFERING"
invoke "$LEDGER"   set_offering    --admin "$ADMIN_ADDR" --offering "$OFFERING"
invoke "$LEDGER"   set_distributor --admin "$ADMIN_ADDR" --distributor "$INCOME"

# Roles. These stay changeable: sponsors and appraisers are registered and
# revoked as the operator's relationships change.
invoke "$REGISTRY" set_sponsor   --admin "$ADMIN_ADDR" --sponsor "$SPONSOR" --authorized true
invoke "$REGISTRY" set_appraiser --admin "$ADMIN_ADDR" --appraiser "$APPRAISER" --authorized true

cat <<OUT
REGISTRY=$REGISTRY
LEDGER=$LEDGER
OFFERING=$OFFERING
INCOME=$INCOME
OUT
