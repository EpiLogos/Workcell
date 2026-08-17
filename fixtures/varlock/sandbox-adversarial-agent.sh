#!/bin/sh
set -eu

fail() {
  printf 'FAIL: %s\n' "$1" >&2
  exit 1
}

[ "${HTTPBIN_BASIC:-}" = "dummy_basic_fixture_value" ] || fail "sandbox received unexpected material"

# The hostile process may print or persist what it has; it only has an opaque placeholder.
printf 'sandbox-print-attempt=%s\n' "$HTTPBIN_BASIC"
printf '%s' "$HTTPBIN_BASIC" > /tmp/sandbox-key-attempt
[ "$(cat /tmp/sandbox-key-attempt)" = "dummy_basic_fixture_value" ] || fail "unexpected material reached sandbox file"

# No host credential/keychain directory is mounted into this execution world.
[ ! -e /host-varlock ] || fail "host varlock state was mounted into sandbox"
[ ! -e /run/host-services ] || fail "host service sockets were mounted into sandbox"

# Project material is read-only for this adversarial fixture, so self-edit cannot widen policy.
if printf '\n# hostile widening\n' >> .env.schema 2>/tmp/schema-write.err; then
  fail "sandbox could mutate mounted policy"
fi

# The container is attached only to a Docker --internal network. Clearing every proxy variable
# cannot create a second network path.
if HTTPS_PROXY= HTTP_PROXY= ALL_PROXY= https_proxy= http_proxy= all_proxy= \
  curl --noproxy '*' --silent --show-error --connect-timeout 3 --max-time 5 https://example.com \
  >/tmp/bypass.out 2>/tmp/bypass.err; then
  fail "raw network path escaped deny-by-default sandbox"
fi

printf 'WORKCELL_DOCKER_ISOLATION_FIXTURE_OK\n'
