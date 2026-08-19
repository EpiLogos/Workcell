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
WORKCELL_SOURCE_REF="${WORKCELL_SOURCE_REF:-agent/workcell-inhabitable-finish}"

usage() {
  cat <<'EOF'
usage: exe-dev-workcell-bootstrap.sh --vm NAME [--create] [--destroy-after]
                                     [--local-port PORT] [--remote-port PORT]
                                     [--workcell-ref REF] [--source-ref REF]

Creates or selects an exe.dev VM through exe.dev's SSH management API,
builds the selected Workcell source ref on that VM, starts the Workcell
Control Service on VM loopback, reaches it through an ordinary SSH tunnel,
and performs a workcell.control/v1 discovery probe from the local machine.

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
    --source-ref)
      WORKCELL_SOURCE_REF="${2:?--source-ref requires a value}"
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
[[ "$VM_NAME" =~ ^[A-Za-z0-9._-]+$ ]] || { echo "unsafe VM name" >&2; exit 2; }
[[ "$WORKCELL_SOURCE_REF" =~ ^[A-Za-z0-9._/-]+$ ]] || { echo "unsafe Workcell source ref" >&2; exit 2; }
[[ "$LOCAL_PORT" =~ ^[0-9]+$ && "$REMOTE_PORT" =~ ^[0-9]+$ ]] || { echo "ports must be numeric" >&2; exit 2; }
for required_command in ssh scp cargo python3; do
  command -v "$required_command" >/dev/null || {
    echo "required command not found: $required_command" >&2
    exit 2
  }
done

select_vm() {
  local name="$1"
  python3 -c '
import json, sys
name = sys.argv[1]
data = json.load(sys.stdin)
for vm in data.get("vms", []):
    if vm.get("vm_name") == name:
        print(json.dumps(vm))
        raise SystemExit(0)
raise SystemExit(3)
' "$name"
}

inventory="$(ssh exe.dev ls --json)"
vm_json="$(printf '%s' "$inventory" | select_vm "$VM_NAME" || true)"

if [[ -z "$vm_json" ]]; then
  if [[ "$CREATE" -ne 1 ]]; then
    echo "exe.dev VM '$VM_NAME' does not exist; pass --create to create it" >&2
    exit 3
  fi
  ssh exe.dev new --name="$VM_NAME" --json >/dev/null
  inventory="$(ssh exe.dev ls --json)"
  vm_json="$(printf '%s' "$inventory" | select_vm "$VM_NAME")"
fi

vm_fields="$(printf '%s' "$vm_json" | python3 -c '
import json, sys
vm = json.load(sys.stdin)
values = [str(vm.get(key, "")) for key in ("ssh_dest", "https_url", "region", "vm_name")]
print("\t".join(values))
')"
IFS=$'\t' read -r SSH_DEST HTTPS_URL REGION ACTUAL_VM_NAME <<<"$vm_fields"
[[ -n "$SSH_DEST" ]] || { echo "exe.dev did not report ssh_dest for '$VM_NAME'" >&2; exit 4; }

# The local machine only needs the transport probe. The service is built on
# the target VM so a macOS/ARM workstation does not accidentally upload an
# incompatible binary to a Linux/x86_64 or Linux/ARM VM.
cargo build --release -p epilogos-workcell-cli --bin workcell-control-client

ssh "$SSH_DEST" "mkdir -p ~/$REMOTE_STATE_ROOT"

TOKEN="${WORKCELL_CONTROL_TOKEN:-$(python3 -c 'import secrets; print(secrets.token_hex(32))')}"
token_file="$(mktemp)"
printf '%s\n' "$TOKEN" >"$token_file"
chmod 600 "$token_file"
scp "$token_file" "$SSH_DEST:~/$REMOTE_STATE_ROOT/control.token" >/dev/null
rm -f "$token_file"
ssh "$SSH_DEST" "chmod 600 ~/$REMOTE_STATE_ROOT/control.token"

# Build and install exactly the requested Workcell source ref on the VM using
# ordinary source/bootstrap tooling. Host acquisition remains outside Workcell.
ssh "$SSH_DEST" bash -s -- "$WORKCELL_SOURCE_REF" "$REMOTE_STATE_ROOT" <<'REMOTE_BUILD'
set -eu
source_ref="$1"
state_root="$2"
repo="$HOME/$state_root/source"

if ! command -v git >/dev/null 2>&1; then
  echo "remote VM requires git for Workcell source bootstrap" >&2
  exit 21
fi
if ! command -v cargo >/dev/null 2>&1; then
  if ! command -v curl >/dev/null 2>&1; then
    echo "remote VM requires cargo or curl for rustup bootstrap" >&2
    exit 22
  fi
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
    | sh -s -- -y --profile minimal
  export PATH="$HOME/.cargo/bin:$PATH"
fi

if [ -d "$repo/.git" ]; then
  git -C "$repo" fetch --prune origin
else
  rm -rf "$repo"
  git clone https://github.com/EpiLogos/Workcell.git "$repo"
fi
git -C "$repo" fetch origin "$source_ref"
git -C "$repo" checkout --detach FETCH_HEAD
cargo build --manifest-path "$repo/Cargo.toml" --release \
  -p epilogos-workcell-cli --bin workcell-control-service
mkdir -p "$HOME/.local/bin"
cp "$repo/target/release/workcell-control-service" \
  "$HOME/.local/bin/workcell-control-service"
REMOTE_BUILD

# The target process listens only on VM loopback. The SSH tunnel is a material
# carrier; the bytes crossing it are still workcell.control/v1.
ssh "$SSH_DEST" bash -s -- "$REMOTE_STATE_ROOT" "$REMOTE_PORT" "$WORKCELL_REF" <<'REMOTE_START'
set -eu
state_root="$1"
remote_port="$2"
workcell_ref="$3"
token="$(cat "$HOME/$state_root/control.token")"

if [ -f "$HOME/$state_root/control.pid" ]; then
  old_pid="$(cat "$HOME/$state_root/control.pid")"
  if kill -0 "$old_pid" 2>/dev/null; then
    kill "$old_pid"
  fi
fi
nohup env WORKCELL_CONTROL_TOKEN="$token" \
  "$HOME/.local/bin/workcell-control-service" \
  --listen "127.0.0.1:$remote_port" \
  --state-root "$HOME/$state_root/state" \
  --workcell-ref "$workcell_ref" \
  >"$HOME/$state_root/control.log" 2>&1 < /dev/null &
echo $! >"$HOME/$state_root/control.pid"
REMOTE_START

ssh -o ExitOnForwardFailure=yes -N \
  -L "127.0.0.1:$LOCAL_PORT:127.0.0.1:$REMOTE_PORT" \
  "$SSH_DEST" &
TUNNEL_PID=$!
probe_file="$(mktemp)"
cleanup() {
  rm -f "$probe_file"
  kill "$TUNNEL_PID" 2>/dev/null || true
}
trap cleanup EXIT INT TERM

probe_ok=0
for _ in {1..20}; do
  if WORKCELL_CONTROL_TOKEN="$TOKEN" \
    target/release/workcell-control-client \
      --endpoint "127.0.0.1:$LOCAL_PORT" --json discover >"$probe_file" 2>/dev/null; then
    probe_ok=1
    break
  fi
  sleep 1
done
[[ "$probe_ok" -eq 1 ]] || {
  echo "remote Workcell Control Service did not become discoverable" >&2
  exit 5
}

python3 - \
  "$EXE_DEV_SOURCE_CHECKED" "$EXE_DEV_API_SOURCE" "$EXE_DEV_NEW_SOURCE" \
  "$ACTUAL_VM_NAME" "$SSH_DEST" "$HTTPS_URL" "$REGION" \
  "$WORKCELL_REF" "$WORKCELL_SOURCE_REF" "$LOCAL_PORT" "$REMOTE_PORT" "$probe_file" <<'PY'
import json, sys
(
    checked, api_source, new_source, vm_name, ssh_dest, https_url, region,
    workcell_ref, source_ref, local_port, remote_port, probe_file,
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
    "workcell_install": {
        "repository": "EpiLogos/Workcell",
        "requested_source_ref": source_ref,
        "build_location": "target-vm",
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
