#!/usr/bin/env bash
#
# readonly-probe.sh
#
# Tests whether your CURRENT AWS credentials can create + attach + assume a
# read-only IAM role -- the exact mechanism `rl --readonly` would use to mint
# reduced-privilege credentials.
#
# It creates a uniquely-named throwaway role and ALWAYS cleans it up afterwards
# (on success, failure, or Ctrl-C) via an EXIT trap. Nothing persists.
#
# Usage:
#   1. Select credentials for the account you want to test (e.g. `rl`).
#   2. bash readonly-probe.sh
#
# Run it once per account you care about -- permissions and SCPs vary per account.

set -u

READONLY_ARN="arn:aws:iam::aws:policy/ReadOnlyAccess"
ROLE_NAME="roleman-perms-test-$$-${RANDOM}"
ROLE_CREATED=0

# ---------------------------------------------------------------------------
# Cleanup: detach every managed policy, delete every inline policy, then delete
# the role. Safe to run even if the role was never created. Registered on EXIT
# so it runs no matter how the script terminates.
# ---------------------------------------------------------------------------
cleanup() {
  [ "$ROLE_CREATED" -eq 1 ] || exit
  echo
  echo "-> cleanup: removing ${ROLE_NAME}"
  for arn in $(aws iam list-attached-role-policies --role-name "$ROLE_NAME" \
        --query 'AttachedPolicies[].PolicyArn' --output text 2>/dev/null); do
    aws iam detach-role-policy --role-name "$ROLE_NAME" --policy-arn "$arn" >/dev/null 2>&1
  done
  for name in $(aws iam list-role-policies --role-name "$ROLE_NAME" \
        --query 'PolicyNames[]' --output text 2>/dev/null); do
    aws iam delete-role-policy --role-name "$ROLE_NAME" --policy-name "$name" >/dev/null 2>&1
  done
  if aws iam delete-role --role-name "$ROLE_NAME" >/dev/null 2>&1; then
    echo "   cleaned up ${ROLE_NAME}"
  else
    echo "   WARNING: could not delete ${ROLE_NAME}; remove it manually:"
    echo "     aws iam delete-role --role-name ${ROLE_NAME}"
  fi
}
trap cleanup EXIT INT TERM

# ---------------------------------------------------------------------------
echo "== roleman --readonly permission probe =="

CALLER_ARN=$(aws sts get-caller-identity --query Arn --output text 2>/dev/null) || {
  echo "ERROR: no valid AWS credentials in this shell."
  echo "       Select an account first (e.g. \`rl\`) and re-run."
  exit 1
}
ACCT=$(aws sts get-caller-identity --query Account --output text)
echo "Account: ${ACCT}"
echo "Caller:  ${CALLER_ARN}"
echo

# Trust policy is exactly what roleman would create: allow this account's SSO
# permission-set roles (any suffix) to assume the downscope role.
TRUST=$(printf '{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"AWS":"arn:aws:iam::%s:root"},"Action":"sts:AssumeRole","Condition":{"ArnLike":{"aws:PrincipalArn":"arn:aws:iam::%s:role/aws-reserved/sso.amazonaws.com/*"}}}]}' "$ACCT" "$ACCT")

CREATE_OK=0
ATTACH_OK=0

echo "-> iam:CreateRole"
if ERR=$(aws iam create-role \
      --role-name "$ROLE_NAME" \
      --assume-role-policy-document "$TRUST" \
      --description "roleman permission probe (safe to delete)" \
      --max-session-duration 3600 \
      --tags Key=roleman,Value=permission-probe \
      2>&1 1>/dev/null); then
  ROLE_CREATED=1
  CREATE_OK=1
  echo "   OK -- role creation allowed"
else
  echo "   DENIED:"
  printf '   %s\n' "$ERR"
fi

if [ "$CREATE_OK" -eq 1 ]; then
  echo "-> iam:AttachRolePolicy (ReadOnlyAccess)"
  if ERR=$(aws iam attach-role-policy \
        --role-name "$ROLE_NAME" \
        --policy-arn "$READONLY_ARN" \
        2>&1 1>/dev/null); then
    ATTACH_OK=1
    echo "   OK -- ReadOnlyAccess attached"
  else
    echo "   DENIED:"
    printf '   %s\n' "$ERR"
  fi

  echo "-> sts:AssumeRole (waiting 8s for IAM propagation)"
  sleep 8
  if OUT=$(aws sts assume-role \
        --role-arn "arn:aws:iam::${ACCT}:role/${ROLE_NAME}" \
        --role-session-name roleman-probe \
        --query 'AssumedRoleUser.Arn' --output text 2>&1); then
    echo "   OK -- assumed as ${OUT}"
  else
    echo "   could not assume yet (usually just propagation lag, not a permission issue):"
    printf '   %s\n' "$OUT"
  fi
fi

echo
echo "== result =="
if [ "$CREATE_OK" -eq 1 ] && [ "$ATTACH_OK" -eq 1 ]; then
  echo "SUPPORTED: this account can use the on-demand read-only role approach."
elif [ "$CREATE_OK" -eq 1 ]; then
  echo "PARTIAL: you can create roles but not attach managed policies here."
  echo "         roleman would use an inline read-only policy instead."
else
  echo "BLOCKED: role creation is denied here (likely an SCP or permission boundary)."
  echo "         The on-demand role approach will not work for this account."
fi
# cleanup() runs automatically on exit.
