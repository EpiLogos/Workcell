#!/usr/bin/env bash
set -euo pipefail

# Caller selects actual native placement, or the explicit source-pinned hosted
# staging exception. No directory shape/default temp invents a Project or NOW.
WORKCELL_TEST_ARTIFACT_ROOT="$(python3 - "$PWD" <<'PY_ARTIFACT'
import json
import os
import pathlib
import stat
import tempfile
import sys
repository = pathlib.Path(sys.argv[1]).resolve(strict=True)
central_context = None
for ancestor in (repository, *repository.parents):
    try:
        marker = (ancestor / "Control/user/native-action-authority.json").stat()
    except FileNotFoundError:
        continue
    if not stat.S_ISREG(marker.st_mode):
        raise SystemExit("compiled Central context marker is not a regular file")
    central_context = ancestor
    break
def required(name):
    value = os.environ.get(name)
    if not value:
        raise SystemExit("required artifact admission input is absent: " + name)
    return value
original = pathlib.Path(required("WORKCELL_TEST_ARTIFACT_ROOT"))
source = pathlib.Path(required("WORKCELL_TEST_ARTIFACT_OWNER_SOURCE"))
owner_ref = required("WORKCELL_TEST_ARTIFACT_OWNER_REF")
admission = required("WORKCELL_TEST_ARTIFACT_ADMISSION")
if not original.is_absolute() or not source.is_absolute():
    raise SystemExit("artifact placement and owner source must be absolute")
base = original.resolve(strict=True)
if not base.is_dir() or base.stat().st_uid != os.getuid():
    raise SystemExit("artifact root must be an existing coordinator-owned directory")
def identity(value):
    return (value.st_dev, value.st_ino, value.st_size, value.st_mtime_ns, value.st_ctime_ns)
fd = os.open(source, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC)
with os.fdopen(fd, "rb") as held:
    initial = os.fstat(held.fileno())
    if not stat.S_ISREG(initial.st_mode) or initial.st_nlink != 1 or initial.st_size > 65536:
        raise SystemExit("artifact owner source must be a bounded regular single-link file")
    data = held.read(65537)
    named = source.lstat()
    if len(data) > 65536 or not stat.S_ISREG(named.st_mode) or identity(initial) != identity(named) or identity(initial) != identity(os.fstat(held.fileno())) or source.resolve(strict=True) != source:
        raise SystemExit("artifact owner source changed or is not the selected canonical source")
def contained(root):
    return base == root or root in base.parents
if admission == "central-clearing":
    record = json.loads(data)
    clearing = source.parent
    central = clearing.parents[4]
    native_id = clearing.name
    expected = central / "Control/agents/now/clearings" / native_id / "now.json"
    if central_context != central:
        raise SystemExit("clearing source is outside the actual compiled Central working context")
    if source != expected or owner_ref != "central:now:control:root:" + native_id or record.get("schema") != "central.now-clearing/v1" or record.get("now_ref") != owner_ref or record.get("source_ref") != "central:source:control:root:Control/agents/now/clearings/" + native_id + "/now.json" or record.get("scope_ref") != "control:root" or not isinstance(record.get("policy_revision_at_allocation"), str) or not record["policy_revision_at_allocation"] or not isinstance(record.get("participant_refs"), list) or not record["participant_refs"] or not contained((clearing / "T").resolve(strict=True)):
        raise SystemExit("artifact root is not inside the selected actual native clearing T")
elif admission == "product-scratch":
    record = json.loads(data)
    project = source.parent.parent
    native_id = record.get("project_id")
    if not isinstance(native_id, str) or not native_id:
        raise SystemExit("selected product source has no native project identity")
    actual_product_context = project.parent == central_context / "Work" if central_context is not None else project == repository
    if not actual_product_context:
        raise SystemExit("product scratch source is outside the actual compiled product owner context")
    if record.get("schema") != "central.project/v1" or not isinstance(native_id, str) or not native_id or record.get("human_source") != "ProjectCentral/user" or owner_ref != "project:" + native_id or source != project / "ProjectCentral/project.json" or not contained((project / "ProjectCentral/now/tmp").resolve(strict=True)):
        raise SystemExit("artifact root is not inside the selected actual product scratch owner")
    # Native product scratch is not a fabricated Run allocation.
elif admission == "hosted-runner":
    workspace = pathlib.Path(required("GITHUB_WORKSPACE")).resolve(strict=True)
    if os.environ.get("GITHUB_ACTIONS") != "true" or os.environ.get("CI") != "true" or not os.environ.get("GITHUB_RUN_ID") or not os.environ.get("GITHUB_REPOSITORY") or workspace != repository or base != (workspace / "evidence/native-processes").resolve(strict=True) or source != workspace / "evidence/source-commit.txt" or data.decode("utf-8").strip() != owner_ref or len(owner_ref) != 40 or any(value not in "0123456789abcdefABCDEF" for value in owner_ref):
        raise SystemExit("artifact root is not the explicit source-pinned hosted staging exception")
else:
    raise SystemExit("unrecognised artifact admission kind")
# This caller creates only an exclusive child inside its admitted native route.
# Hosted staging remains at the exact existing job-owned evidence root.
print(base if admission == "hosted-runner" else tempfile.mkdtemp(prefix="workcell-native-tests-", dir=base))
PY_ARTIFACT
)"
export WORKCELL_TEST_ARTIFACT_ROOT
printf 'Native verification evidence: %s\n' "$WORKCELL_TEST_ARTIFACT_ROOT"

bash -n scripts/exe-dev-workcell-bootstrap.sh
bash -n scripts/stage-remote-file-share.sh
test "$(id -u)" -ne 0
command -v python3
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
set -o pipefail
cargo test --locked -p epilogos-workcell-runtime --test external_instance_binding -- --list 2>&1 | tee "$WORKCELL_TEST_ARTIFACT_ROOT/external-instance-list.txt"
test "$(grep -c ': test$' "$WORKCELL_TEST_ARTIFACT_ROOT/external-instance-list.txt")" -eq 19
cargo test --locked -p epilogos-workcell-runtime --test external_instance_binding -- --test-threads=1 2>&1 | tee "$WORKCELL_TEST_ARTIFACT_ROOT/external-instance-tests.txt"
grep -F '19 passed; 0 failed; 0 ignored;' "$WORKCELL_TEST_ARTIFACT_ROOT/external-instance-tests.txt"
cargo test --locked -p epilogos-workcell-runtime --lib bounded_process::tests -- --list 2>&1 | tee "$WORKCELL_TEST_ARTIFACT_ROOT/bounded-process-list.txt"
test "$(grep -c ': test$' "$WORKCELL_TEST_ARTIFACT_ROOT/bounded-process-list.txt")" -eq 4
cargo test --locked -p epilogos-workcell-runtime --lib bounded_process::tests -- --test-threads=1 2>&1 | tee "$WORKCELL_TEST_ARTIFACT_ROOT/bounded-process-tests.txt"
grep -F '4 passed; 0 failed; 0 ignored;' "$WORKCELL_TEST_ARTIFACT_ROOT/bounded-process-tests.txt"
cargo test --locked -p epilogos-workcell-cli --test bounded_write_boundary -- --list 2>&1 | tee "$WORKCELL_TEST_ARTIFACT_ROOT/bounded-write-boundary-list.txt"
test "$(grep -c ': test$' "$WORKCELL_TEST_ARTIFACT_ROOT/bounded-write-boundary-list.txt")" -eq 3
cargo test --locked -p epilogos-workcell-cli --test bounded_write_boundary -- --test-threads=1 2>&1 | tee "$WORKCELL_TEST_ARTIFACT_ROOT/bounded-write-boundary-tests.txt"
grep -F '3 passed; 0 failed; 0 ignored;' "$WORKCELL_TEST_ARTIFACT_ROOT/bounded-write-boundary-tests.txt"
cargo test --locked -p epilogos-workcell-cli --bin workcell admission_inputs::tests -- --list 2>&1 | tee "$WORKCELL_TEST_ARTIFACT_ROOT/admission-inputs-list.txt"
test "$(grep -c ': test$' "$WORKCELL_TEST_ARTIFACT_ROOT/admission-inputs-list.txt")" -eq 4
cargo test --locked -p epilogos-workcell-cli --bin workcell admission_inputs::tests -- --test-threads=1 2>&1 | tee "$WORKCELL_TEST_ARTIFACT_ROOT/admission-inputs-tests.txt"
grep -F '4 passed; 0 failed; 0 ignored;' "$WORKCELL_TEST_ARTIFACT_ROOT/admission-inputs-tests.txt"
cargo test --locked -p epilogos-workcell-runtime --lib bounded_process::status_only_tests -- --list 2>&1 | tee "$WORKCELL_TEST_ARTIFACT_ROOT/status-only-list.txt"
test "$(grep -c ': test$' "$WORKCELL_TEST_ARTIFACT_ROOT/status-only-list.txt")" -eq 3
cargo test --locked -p epilogos-workcell-runtime --lib bounded_process::status_only_tests -- --test-threads=1 2>&1 | tee "$WORKCELL_TEST_ARTIFACT_ROOT/status-only-tests.txt"
grep -F '3 passed; 0 failed; 0 ignored;' "$WORKCELL_TEST_ARTIFACT_ROOT/status-only-tests.txt"
cargo test --locked -p epilogos-workcell-runtime --lib external_service::legacy_command_tests -- --list 2>&1 | tee "$WORKCELL_TEST_ARTIFACT_ROOT/legacy-command-list.txt"
test "$(grep -c ': test$' "$WORKCELL_TEST_ARTIFACT_ROOT/legacy-command-list.txt")" -eq 3
cargo test --locked -p epilogos-workcell-runtime --lib external_service::legacy_command_tests -- --test-threads=1 2>&1 | tee "$WORKCELL_TEST_ARTIFACT_ROOT/legacy-command-tests.txt"
grep -F '3 passed; 0 failed; 0 ignored;' "$WORKCELL_TEST_ARTIFACT_ROOT/legacy-command-tests.txt"
cargo test --locked -p epilogos-workcell-cli --bin workcell combined::local_cli::source_io_tests -- --list 2>&1 | tee "$WORKCELL_TEST_ARTIFACT_ROOT/source-io-list.txt"
test "$(grep -c ': test$' "$WORKCELL_TEST_ARTIFACT_ROOT/source-io-list.txt")" -eq 3
cargo test --locked -p epilogos-workcell-cli --bin workcell combined::local_cli::source_io_tests -- --test-threads=1 2>&1 | tee "$WORKCELL_TEST_ARTIFACT_ROOT/source-io-tests.txt"
grep -F '3 passed; 0 failed; 0 ignored;' "$WORKCELL_TEST_ARTIFACT_ROOT/source-io-tests.txt"
