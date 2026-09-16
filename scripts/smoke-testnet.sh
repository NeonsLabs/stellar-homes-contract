#!/usr/bin/env bash
# Drive one mortgage end to end against a live network: register a property,
# verify its title, value it, fund the pool, approve a loan, sign off the
# foundation, draw the tranche and take a repayment.
#
# Everything here is a real transaction. Run it against testnet.
#
# Required environment: the three contract ids printed by deploy.sh.
#   REGISTRY LENDING MORTGAGE
#
# Optional environment:
#   NETWORK      stellar network name                    (default: testnet)
#   ADMIN        admin identity, also the underwriter    (default: sh-admin)
#   TRUSTEE      trustee identity, already registered    (default: sh-trustee)
#   ORACLE       oracle identity, already registered     (default: sh-oracle)
#   BORROWER     borrower identity, funded with USDC     (default: sh-borrower)
#   INVESTOR     investor identity, funded with USDC     (default: sh-investor)
set -euo pipefail

NETWORK=${NETWORK:-testnet}
ADMIN=${ADMIN:-sh-admin}
TRUSTEE=${TRUSTEE:-sh-trustee}
ORACLE=${ORACLE:-sh-oracle}
BORROWER=${BORROWER:-sh-borrower}
INVESTOR=${INVESTOR:-sh-investor}
: "${REGISTRY:?set REGISTRY}"
: "${LENDING:?set LENDING}"
: "${MORTGAGE:?set MORTGAGE}"

if [ "$NETWORK" = "mainnet" ]; then
  echo "This script draws a real mortgage and spends real money. Not on mainnet." >&2
  exit 1
fi

TRUSTEE_ADDR=$(stellar keys address "$TRUSTEE")
ORACLE_ADDR=$(stellar keys address "$ORACLE")
BORROWER_ADDR=$(stellar keys address "$BORROWER")
INVESTOR_ADDR=$(stellar keys address "$INVESTOR")
ADMIN_ADDR=$(stellar keys address "$ADMIN")

call() {
  local id=$1 source=$2; shift 2
  stellar contract invoke --id "$id" --source-account "$source" --network "$NETWORK" -- "$@"
}

# Distinct document hashes per run, so repeated smoke tests do not collide.
TITLE=$(head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n')
SURVEY=$(head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n')
EVIDENCE=$(head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n')

VALUATION=1000000000   # 100 USDC at 7 decimals
PRINCIPAL=500000000    # 50 USDC — comfortably inside the 80% ceiling
TERM=120
RATE_BPS=850

echo "==> Investor funds the pool"
call "$LENDING" "$INVESTOR" deposit \
  --investor "$INVESTOR_ADDR" --amount 1000000000 >/dev/null

echo "==> Trustee registers a property"
PROPERTY=$(call "$REGISTRY" "$TRUSTEE" submit_property \
  --trustee "$TRUSTEE_ADDR" --title_hash "$TITLE" --survey_doc_hash "$SURVEY")
echo "    property $PROPERTY"

echo "==> Oracle verifies the title and publishes a valuation"
call "$REGISTRY" "$ORACLE" verify_title \
  --oracle "$ORACLE_ADDR" --property_id "$PROPERTY" >/dev/null
call "$REGISTRY" "$ORACLE" set_valuation \
  --oracle "$ORACLE_ADDR" --property_id "$PROPERTY" --usdc_value "$VALUATION" >/dev/null

echo "==> Borrower applies"
LOAN=$(call "$MORTGAGE" "$BORROWER" apply \
  --borrower "$BORROWER_ADDR" --property_id "$PROPERTY" --principal "$PRINCIPAL" \
  --term_months "$TERM" --rate_bps "$RATE_BPS")
echo "    mortgage $LOAN"

echo "==> Underwriter approves, committing the facility"
call "$MORTGAGE" "$ADMIN" approve \
  --underwriter "$ADMIN_ADDR" --mortgage_id "$LOAN" >/dev/null
echo -n "    pool available: "
call "$LENDING" "$ADMIN" available

echo "==> Trustee submits foundation evidence; oracle signs it off"
call "$REGISTRY" "$TRUSTEE" submit_milestone_evidence \
  --trustee "$TRUSTEE_ADDR" --property_id "$PROPERTY" --stage 0 \
  --evidence_hash "$EVIDENCE" >/dev/null
call "$REGISTRY" "$ORACLE" verify_milestone \
  --oracle "$ORACLE_ADDR" --property_id "$PROPERTY" --stage 0 >/dev/null

echo "==> Anyone releases the tranche (the admin stands in here)"
call "$MORTGAGE" "$ADMIN" disburse --mortgage_id "$LOAN" --stage 0

echo -n "==> Payoff amount: "
call "$MORTGAGE" "$BORROWER" payoff_amount --mortgage_id "$LOAN"

echo "==> Borrower clears the loan outright"
PAYOFF=$(call "$MORTGAGE" "$BORROWER" payoff_amount --mortgage_id "$LOAN")
call "$MORTGAGE" "$BORROWER" repay --mortgage_id "$LOAN" --amount "$PAYOFF" >/dev/null

echo -n "==> Property status: "
call "$REGISTRY" "$ADMIN" get_status_of --property_id "$PROPERTY"

echo "Smoke test passed for property $PROPERTY, mortgage $LOAN."
