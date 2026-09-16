#!/usr/bin/env bash
# Hand the admin role on all four contracts from the deploying key to a
# multisig account.
#
# The handover is two-step on every contract: this script proposes, and the
# multisig must then accept from its own account. Nothing changes until it
# does, so a mistyped address cannot lock the protocol out.
#
# Required environment:
#   REGISTRY LEDGER OFFERING INCOME  contract ids from deploy.sh
#   MULTISIG                         address of the account taking over
#
# Optional environment:
#   NETWORK  stellar network name   (default: testnet)
#   ADMIN    current admin identity (default: sh-admin)
set -euo pipefail

NETWORK=${NETWORK:-testnet}
ADMIN=${ADMIN:-sh-admin}
: "${REGISTRY:?set REGISTRY}"
: "${LEDGER:?set LEDGER}"
: "${OFFERING:?set OFFERING}"
: "${INCOME:?set INCOME}"
: "${MULTISIG:?set MULTISIG to the address taking over as admin}"

ADMIN_ADDR=$(stellar keys address "$ADMIN")
if [ "$ADMIN_ADDR" = "$MULTISIG" ]; then
  echo "MULTISIG is the current admin; nothing to hand over." >&2
  exit 1
fi

for id in "$REGISTRY" "$LEDGER" "$OFFERING" "$INCOME"; do
  echo "==> Proposing $MULTISIG as admin of $id"
  stellar contract invoke --id "$id" --source-account "$ADMIN" --network "$NETWORK" \
    -- propose_admin --admin "$ADMIN_ADDR" --new_admin "$MULTISIG" >/dev/null
done

cat <<OUT

Proposed on all four contracts. Nothing has changed yet.

The multisig must now accept on each, signing as $MULTISIG:

  for id in $REGISTRY $LEDGER $OFFERING $INCOME; do
    stellar contract invoke --id \$id --source-account <multisig> \\
      --network $NETWORK -- accept_admin --new_admin $MULTISIG
  done

Verify afterwards with get_admin on each contract before retiring the old key.
OUT
