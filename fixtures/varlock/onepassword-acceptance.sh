#!/bin/sh
set -eu

: "${OP_SERVICE_ACCOUNT_TOKEN:?set a least-privilege 1Password service-account token}"
: "${OP_TEST_SECRET_REF:?set one test secret reference such as op://vault/item/field}"

case "$OP_TEST_SECRET_REF" in
  op://*) ;;
  *)
    printf 'OP_TEST_SECRET_REF must be an op:// secret reference\n' >&2
    exit 2
    ;;
esac

command -v varlock >/dev/null 2>&1 || {
  printf 'varlock is required\n' >&2
  exit 2
}

version="$(varlock --version 2>/dev/null || true)"
printf '%s\n' "$version" | grep -q '1.16.1' || {
  printf 'expected Varlock 1.16.1, got: %s\n' "$version" >&2
  exit 2
}

root="$(mktemp -d)"
trap 'rm -rf "$root"' EXIT INT TERM

cat > "$root/.env.schema" <<EOF
# @plugin(@varlock/1password-plugin@2.0.0)
# @initOp(token=\$OP_TOKEN, allowAppAuth=false)
# ---
# @type=opServiceAccountToken @sensitive @internal
OP_TOKEN=

# @sensitive @required
TARGET_SECRET=op($OP_TEST_SECRET_REF)
EOF

(
  cd "$root"
  OP_TOKEN="$OP_SERVICE_ACCOUNT_TOKEN" varlock run -- sh -eu -c '
    test -n "${TARGET_SECRET:-}"
    test -z "${OP_TOKEN:-}"
    printf "VARLOCK_1PASSWORD_PROVIDER_ACCEPTANCE_OK\n"
  '
)
