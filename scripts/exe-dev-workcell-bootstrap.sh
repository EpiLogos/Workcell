#!/usr/bin/env bash
set -euo pipefail

# Source gate re-checked 2026-08-19 against the official exe.dev docs:
#   https://exe.dev/docs/api
#   https://exe.dev/docs/https-api
#   https://exe.dev/docs/cli-new
# The management API is SSH (`ssh exe.dev ... --json`); the HTTPS `/exec`
# surface carries the same command API. Neither is the Workcell Control
# Protocol. VM/SSH/HTTPS/region values below are deployment provenance only.
EXE_DEV_SOURCE_CHECKED="2026-08-19"
EXE_DEV_API_SOURCE="https://exe.dev/docs/api"
EXE_DEV_NEW_SOURCE="https://exe.dev/docs/cli-new"

VM_NAME=""
CREATE=0
DESTROY_AFTER=0
REMOTE_PORT=7777
LOCAL_PORT=17777
REMOTE_STATE_ROOT=".local/state/epilogos-workcell"
WORKCELL_REF="workcell:exe-dev-reference"

usage() {
  cat <<'EOF'
usage: exe-dev-workcell-bootstrap.sh --vm NAME [--create] [--destroy-after]
                                     [--local-port PORT] [--remote-port PORT]
                                     [--workcell-ref REF]

Creates or selects an exe.dev VM through exe.dev's SSH management API,
installs the current Workcell Control Service binary, reaches it through an
ordinary SSH tunnel, and performs a workcell.control/v1 discovery probe.

The VM/SSH/tunnel are deployment/bootstrap material. The Workcell Control
Service remains the provider-neutral remote control protocol.
EOF
}

while (($#)); do
  case "$1" in
    --vm)
      VM_NAME="${2:?--vm requires a value}"
      shift 2
      ;;
    --create)
      CREATE=1
      shift
      ;;
    --destroy-after)
      DESTROY_AFTER=1
      shift
      ;;
    --local-port)
      LOCAL_PORT="${2:?--local-port requires a value}"
      shift 2
      ;;
    --remote-port)
      REMOTE_PORT="${2:?--remote-port requires a value}"
      shift 2
      ;;
    --workcell-ref)
      WORKCELL_REF="${2:?--workcell-ref requires a value}"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

[[ -n "$VM_NAME" ]] || { echo "--vm NAME is required" >&2; exit 2; }
for command in ssh scp cargo python3; do
  command -v "$command" >/dev/null || { echo "required command not found: $command" >&2; exit 2; }
done

inventory="$(ssh exe.dev ls --json)"
vm_json="$(python3 - "$VM_NAME" <<'PY' <<<"$inventory"
import json, sys
name = sys.argv[1]
data = json.load(sys.stdin)
for vm in data.get("vms", []):
    if vm.get("vm_name") == name:
        print(json.dumps(vm))
        raise SystemExit(0)
raise SystemExit(3)
PY
)" || true

if [[ -z "$vm_json" ]]; then
  if [[ "$CREATE" -ne 1 ]]; then
    echo "exe.dev VM '$VM_NAME' does not exist; pass --create to create it" >&2
    exit 3
  fi
  ssh exe.dev new --name="$VM_NAME" --json >/dev/null
  inventory="$(ssh exe.dev ls --json)"
  vm_json="$(python3 - "$VM_NAME" <<'PY' <<<"$inventory"
import json, sys
name = sys.argv[1]
data = json.load(sys.stdin)
for vm in data.get("vms", []):
    if vm.get("vm_name") == name:
        print(json.dumps(vm))
        raise SystemExit(0)
raise SystemExit(3)
PY
)"
fi

readarray -t vm_fields < <(python3 - <<'PY' <<<"$vm_json"
import json, sys
vm = json.load(sys.stdin)
for key in ("ssh_dest", "https_url", "region", "vm_name"):
    print(vm.get(key, ""))
PY
)
SSH_DEST="${vm_fields[0]}"
HTTPS_URL="${vm_fields[1]}"
REGION="${vm_fields[2]}"
ACTUAL_VM_NAME="${vm_fields[3]}"
[[ -n "$SSH_DEST" ]] || { echo "exe.dev did not report ssh_dest for '$VM_NAME'" >&2; exit 4; }

cargo build --release -p epilogos-workcell-cli \
  --bin workcell-control-service \
  --bin workcell-control-client

ssh "$SSH_DEST" "mkdir -p ~/.local/bin ~/$REMOTE_STATE_ROOT"
scp target/release/workcell-control-service "$SSH_DEST:~/.local/bin/workcell-control-service"

TOKEN="${WORKCELL_CONTROL_TOKEN:-$(python3 - <<'PY'
import secrets
print(secrets.token_hex(32))
PY
)}"

# The target process listens only on VM loopback. The SSH tunnel is a material
# carrier used to reach it; the bytes crossing that tunnel are workcell.control/v1.
ssh "$SSH_DEST" "set -eu; \
  if [ -f ~/$REMOTE_STATE_ROOT/control.pid ] && kill -0 \$(cat ~/$REMOTE_STATE_ROOT/control.pid) 2>/dev/null; then \
    kill \$(cat ~/$REMOTE_STATE_ROOT/control.pid); \
  fi; \
  nohup env WORKCELL_CONTROL_TOKEN='$TOKEN' ~/.local/bin/workcell-control-service \
    --listen 127.0.0.1:$REMOTE_PORT \
    --state-root ~/$REMOTE_STATE_ROOT/state \
    --workcell-ref '$WORKCELL_REF' \
    > ~/$REMOTE_STATE_ROOT/control.log 2>&1 < /dev/null & \
  echo \$! > ~/$REMOTE_STATE_ROOT/control.pid"

ssh -o ExitOnForwardFailure=yes -N \
  -L "127.0.0.1:$LOCAL_PORT:127.0.0.1:$REMOTE_PORT" \
  "$SSH_DEST" &
TUNNEL_PID=$!
cleanup() {
  kill "$TUNNEL_PID" 2>/dev/null || true
}
trap cleanup EXIT INT TERM

probe_file="$(mktemp)"
trap 'rm -f "$probe_file"; cleanup' EXIT INT TERM
probe_ok=0
for _ in $(seq 1 20); do
  if WORKCELL_CONTROL_TOKEN="$TOKEN" \
    target/release/workcell-control-client \
      --endpoint "127.0.0.1:$LOCAL_PORT" --json discover >"$probe_file" 2>/dev/null; then
    probe_ok=1
    break
  fi
  sleep 1
done
[[ "$probe_ok" -eq 1 ]] || { echo "remote Workcell Control Service did not become discoverable" >&2; exit 5; }

python3 - \
  "$EXE_DEV_SOURCE_CHECKED" "$EXE_DEV_API_SOURCE" "$EXE_DEV_NEW_SOURCE" \
  "$ACTUAL_VM_NAME" "$SSH_DEST" "$HTTPS_URL" "$REGION" \
  "$WORKCELL_REF" "$LOCAL_PORT" "$REMOTE_PORT" "$probe_file" <<'PY'
import json, sys
(
    checked, api_source, new_source, vm_name, ssh_dest, https_url, region,
    workcell_ref, local_port, remote_port, probe_file,
) = sys.argv[1:]
with open(probe_file) as handle:
    discovery = json.load(handle)
print(json.dumps({
    "schema": "workcell.exe-dev-bootstrap-receipt/v1",
    "source_gate": {
        "checked": checked,
        "api": api_source,
        "vm_create": new_source,
    },
    "host_acquisition": {
        "provider": "exe.dev",
        "vm_name": vm_name,
        "ssh_dest": ssh_dest,
        "https_url": https_url,
        "region": region,
    },
    "control_transport": {
        "bootstrap_management": "exe.dev SSH API",
        "carrier": "SSH local tunnel",
        "workcell_protocol": "workcell.control/v1",
        "local_endpoint": f"127.0.0.1:{local_port}",
        "remote_loopback_endpoint": f"127.0.0.1:{remote_port}",
    },
    "workcell": discovery,
    "expected_workcell_ref": workcell_ref,
}, indent=2, sort_keys=True))
PY

if [[ "$DESTROY_AFTER" -eq 1 ]]; then
  cleanup
  trap - EXIT INT TERM
  ssh exe.dev rm "$ACTUAL_VM_NAME" --json >/dev/null
fi
