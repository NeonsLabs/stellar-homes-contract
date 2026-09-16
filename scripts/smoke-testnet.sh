#!/usr/bin/env bash
# Drive one property end to end against a live network: register it, appraise
# it, sell it out, settle, pay rent in and claim it.
#
# Everything here is a real transaction. Run it against testnet.
#
# Required environment: the four contract ids printed by deploy.sh, plus USDC.
#   REGISTRY LEDGER OFFERING INCOME USDC
#
# Optional environment:
#   NETWORK    stellar network name                     (default: testnet)
#   ADMIN      admin identity                           (default: sh-admin)
#   SPONSOR    sponsor identity, already registered     (default: sh-sponsor)
#   APPRAISER  appraiser identity, already registered   (default: sh-appraiser)
#   INVESTOR   investor identity, funded with USDC      (default: sh-investor)
set -euo pipefail

NETWORK=${NETWORK:-testnet}
ADMIN=${ADMIN:-sh-admin}
SPONSOR=${SPONSOR:-sh-sponsor}
APPRAISER=${APPRAISER:-sh-appraiser}
INVESTOR=${INVESTOR:-sh-investor}
: "${REGISTRY:?set REGISTRY}"
: "${LEDGER:?set LEDGER}"
: "${OFFERING:?set OFFERING}"
: "${INCOME:?set INCOME}"

if [ "$NETWORK" = "mainnet" ]; then
  echo "This script sells a property and spends real money. Not on mainnet." >&2
  exit 1
fi

SPONSOR_ADDR=$(stellar keys address "$SPONSOR")
APPRAISER_ADDR=$(stellar keys address "$APPRAISER")
INVESTOR_ADDR=$(stellar keys address "$INVESTOR")

call() {
  local id=$1 source=$2; shift 2
  stellar contract invoke --id "$id" --source-account "$source" --network "$NETWORK" -- "$@"
}

# A distinct document hash per run, so repeated smoke tests do not collide.
DOCS=$(head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n')
TOTAL_SHARES=1000
PRICE=1000000          # 0.1 USDC per share, at 7 decimals
SOFT_CAP=400
DURATION=300           # 5 minutes

echo "==> Registering a property"
PROPERTY=$(call "$REGISTRY" "$SPONSOR" register_property \
  --sponsor "$SPONSOR_ADDR" --document_hash "$DOCS" \
  --total_shares "$TOTAL_SHARES" --price_per_share "$PRICE")
echo "    property $PROPERTY"

echo "==> Publishing an independent valuation"
call "$REGISTRY" "$APPRAISER" publish_valuation \
  --appraiser "$APPRAISER_ADDR" --property_id "$PROPERTY" --valuation 100000000000 >/dev/null

echo "==> Opening the offering"
call "$REGISTRY" "$SPONSOR" open_offering \
  --sponsor "$SPONSOR_ADDR" --property_id "$PROPERTY" >/dev/null
call "$OFFERING" "$SPONSOR" open \
  --sponsor "$SPONSOR_ADDR" --property "$PROPERTY" \
  --min_shares "$SOFT_CAP" --duration_secs "$DURATION" >/dev/null

echo "==> Subscribing for every share, which closes the sale immediately"
call "$OFFERING" "$INVESTOR" subscribe \
  --property "$PROPERTY" --investor "$INVESTOR_ADDR" --shares "$TOTAL_SHARES" >/dev/null

echo "==> Closing (permissionless; the admin stands in for anyone here)"
call "$OFFERING" "$ADMIN" close --property "$PROPERTY"

echo "==> Taking delivery of the shares"
call "$OFFERING" "$INVESTOR" claim_shares \
  --property "$PROPERTY" --investor "$INVESTOR_ADDR"
echo -n "    ledger balance: "
call "$LEDGER" "$INVESTOR" balance_of --property "$PROPERTY" --holder "$INVESTOR_ADDR"

echo "==> Paying a month of rent in"
call "$INCOME" "$SPONSOR" deposit_income \
  --property "$PROPERTY" --from "$SPONSOR_ADDR" --amount 10000000 >/dev/null

echo -n "==> Claimable: "
call "$INCOME" "$INVESTOR" claimable --property "$PROPERTY" --holder "$INVESTOR_ADDR"

echo "==> Claiming it"
call "$INCOME" "$INVESTOR" claim --property "$PROPERTY" --holder "$INVESTOR_ADDR"

echo "Smoke test passed for property $PROPERTY."
