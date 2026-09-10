# Continuous work: material operations (Workcell #72)

This is Workcell's material API, not Central governance or AIKit session behaviour.
The service registration continuation from `workcell/service-provider-registration`
(`499bf81be9d0eed8770f92e31ec7d29c15bce101`) is retained alongside main's development-
world inspection. A service labelled gateway, session or scheduler is still an
ordinary material service; preparing it does not execute an Agent or prove Return.

## Published operations

The existing `workcell.control/v1` transport and `WorkcellControlPlane` remain the
control boundary. `ControlClient` exposes `prepare`, **`inspect`**, `observe`,
**`recover`**, `expose`, `collect`, `release`, and `reconcile`. `recover` returns the
ordinary material-world wire object. New generic trait implementations have an
explicit unsupported default; they do not inherit a pretend recovery operation.

`workcell-control-service --state-root STATE --workcell-ref REF --listen HOST:PORT`
is now an explicitly registered native binary. The zero-daemon CLI remains valid
for one-shot material work, but a provider-process-scoped service that must survive
a client invocation requires this persistent host. A target-owned service uses
its existing native supervisor and may outlive both client and Workcell process.
For remote operations use `workcell --endpoint HOST:PORT --json ...` and
`WORKCELL_CONTROL_TOKEN`. The transport is not TLS: use an authenticated private
network or tunnel. A supplied endpoint never silently falls back to local execution.

Prepare and plan accept `--demand-json FILE` for the complete native demand.
The control codec now preserves every storage tier, access, sharing, capacity,
persistence and retention value. Earlier v1 clients without `storage` retain
empty storage, but a present storage requirement is never silently dropped.
Use `--receipt FILE` for `inspect`, `observe`, `recover` and `release`. Recovery
writes a **new receipt when material identity changes**; it does not overwrite the
old receipt to imply an unchanged process or semantic session.

## NOW and durable service state

Central allocates and authorises the NOW clearing. Workcell binds the existing
exact directory via its existing `StorageProvider` contract. It does not mint a
second NOW identity, infer a human Day, copy a profile or delete source bytes.

`STATE/storage.json`:

```json
{
  "schema": "workcell.directory-storage/v1",
  "directories": [
    {"logical_ref": "now:task-opaque", "path": "/absolute/authorised/NOW/task"}
  ]
}
```

The corresponding `ExecutionDemand.storage.required` entry:

```json
{
  "logical_ref": "now:task-opaque",
  "access": "writable",
  "sharing": "shared",
  "minimum_capacity": null,
  "unit": null,
  "persistence": "external",
  "retention": "preserve"
}
```

Use caller-defined `subjects` for opaque Agent, session, clearing/source, policy,
commission and attempt refs. The Rust entry points are `DirectoryStorage`,
`DirectoryStorageProvider`, and `CollapsedLocalConfig::with_directory_storage`.
Logical store names are matched exactly, not to the first available directory.

This provider supports an **existing shared writable attachment**, subject to
current host permissions. It does not promise an exclusive lease, reserved
capacity, read-only confinement, snapshot or storage migration. Path/inode drift
is detected on use/re-entry. Release only detaches the binding; it never erases,
moves or chmods the directory. A declaration alone is not write confinement.

`STATE/services.json` retains `workcell.service-declaration/v1`, explicit
`provider-process-scoped` versus `target-owned` lifetime, and the current managed
and external providers. Gateway/session/scheduler logical names are caller-owned.
Use the supplied NOW/state directory as the declared service cwd or native state
argument. No Workcell-specific data proxy is placed in front of the service.
Target-native start/stop/restart/status/readiness commands are bounded to ten
seconds. Their arguments, environment and output are not copied into observations.

## Lifetime, restart and re-entry

The durable control host takes a kernel lock for its receipt store. Duplicate
clients preparing the same demand and exact basis receive the same receipt;
changed requirements with the same demand ref are refused. Release tombstones
survive restart, preventing a lost response from reminting released work.

`inspect` is receipt readback, not a live health claim. `observe` probes providers.
`recover` first checks non-service material; it re-observes healthy services and
rematerialises an owned, verifiably gone managed child. New child/material IDs
produce a new material world, retaining all caller subjects and a prior-world
relation. Superseded worlds cannot release their successor's bindings. A live
but unready child, changed declaration, ambiguous surviving PID, lost directory,
unowned stopped service or unsupported provider requires explicit re-resolution.

On Linux the managed direct child is killed when its Workcell host dies. This is
not a claim that arbitrary double-forked grandchildren or non-Linux orphan trees
are supervised. Such lifetimes need a target-native supervisor. External services
are re-entered only against their exact declaration digest; Workcell never stops
an observed service it did not start. An owned service with another active binding
cannot be stopped blindly.

Prepare/recover/release/reconcile write an intent before effects and publish
fsynced receipts. Unknown interrupted effects leave `.pending`/`.writing` evidence
and refuse new material mutations, including a different demand that could reuse
the same target. A completed prepare receipt can resolve its lost-publication
acknowledgement without starting again. **Unknown partial provider effects are
not automatically rolled back or replayed.** Retain that journal and reconcile
actual target-native effects under an authorised local plan; deleting an intent
is not an implementation of recovery. This limitation remains explicit rather
than awarding success to an uncertain receipt.

The TCP server bounds incomplete/trickling request frames. Losing an ordinary
client does not terminate prepared services or discard durable receipts.

## Actual material write protection

`workcell-write-boundary capabilities` reports actual platform eligibility and
coverage. `PreparedWriteBoundary` is the same exported Rust adapter used by the
finite `HostProcessOperationGrant` path; it is not a second authority system.

```text
workcell-write-boundary inspect REQUIREMENTS.json CURRENT_POLICY_REVISION
workcell-write-boundary run REQUIREMENTS.json CURRENT_POLICY_REVISION TIMEOUT_MS -- PROGRAM ARG...
```

Requirements schema `workcell.write-boundary/v1` has **all** these fields:
`policy_ref`, `policy_revision`, `authority_ref`, `writable_paths`,
`protected_paths`, `required_coverage`, `expires_at_unix_ms`, plus `schema`.
Paths must be absolute existing directories; at most 64 of each are accepted.
Permit NOW plus authorised source/worktree/build directories, not NOW alone.
A writable ancestor of a protected directory or `/` is refused. A protected
parent such as Work may contain an explicitly permitted NOW/project subtree.
Revision, expiry and path/object identity are rechecked immediately before exec.

The supported adapter is **unprivileged Linux Landlock ABI >= 3**, covering regular
filesystem `file-content`, `file-creation`, `file-removal`, `rename-link`,
`truncate` and `descendant-processes` from the launched process. Rules are applied
before exec, with null input, piped output and other inherited fds close-on-exec.
A failed kernel application fails spawn; there is no advisory fallback. The CLI
bounds execution to at most 60 seconds and returned output to 64 KiB per stream;
truncation/incomplete pipes/timeouts are explicit and are not successful work.

**Not covered:** metadata chmod/chown/xattr/time, read confidentiality, network or
delegated service writes, pre-existing hardlink aliases, outside processes,
privileged workloads, device ioctls, external mount/rename of granted objects,
and live revocation after launch. Requests requiring any unsupported coverage are
refused. Root, unavailable/blocked Landlock, other platforms, and existing target-
owned processes do not acquire this protection by declaration. A protected target
service must be launched through a genuinely supported boundary by its supervisor.

For an existing exact finite execution grant, use
`HostProcessExecutionProvider::bind_write_boundary(grant_ref, boundary, current_revision)`.
Authority refs must match. Dispatch carries the exact `write_boundary_digest` and
`policy_revision`; an absent/mismatched binding or changed basis is refused before
execution. Consuming a grant still does not attest semantic task completion.
Central/AIKit must supply fresh owner revisions; Workcell cannot authenticate an
invented revision by comparing two caller-provided strings.

Primary kernel contract and limitations:
https://www.kernel.org/doc/html/latest/userspace-api/landlock.html

## Relocation and usage

Same-provider recovery retains subjects and records changed material identity.
Cross-provider or cross-Workcell placement uses the existing placement/discovery
contracts and **a new prepare** with retained caller refs. The old host is never
renamed, an old receipt is not rebound silently, and policy requirements must be
re-resolved at the new site. Directory transfer, live session continuation and
credential relocation remain owner operations, not hidden Workcell side effects.

The existing `workcell.resource-usage/v1` / `instances usage` path is retained:
bounded CPU/RSS intervals, instance/PID/start-identity checks, opaque correlations,
and explicit unknown/unsupported metrics. No fabricated GPU/network/cost totals,
argv or environment inventory are introduced by this work.

## Reproduction and local proving

```bash
cargo test --locked --workspace --all-targets
cargo build --locked -p epilogos-workcell-cli --bins
python3 scripts/caw_native_campaign.py --bin-dir target/debug --output evidence/caw.json
WORKCELL_REQUIRE_LANDLOCK=1 cargo test --locked -p epilogos-workcell-runtime --test write_boundary -- --nocapture
```

The native campaign exercises three actual TCP workload processes through the
compiled Workcell host, concurrent prepare, lost replies, interrupted clients,
host death/restart, inspection, recovery, stale requirement refusal, release,
NOW/source byte retention and real managed-to-target-owned provider substitution.
The workloads are fixtures, **not actual AIKit gateways or Agents**. Two controlled
hosts on one machine prove substitution, not physical second placement.

`scripts/caw_installed_world.py census` records exact binary path/digest/version,
source commit/dirty standing, OS and declaration digests without installing or
modifying anything. `exercise --execute-authorized` consumes an explicit
`workcell.installed-campaign/v1` test-World packet: two placements, distinct
per-host census files, exact Workcell refs/endpoints, credential environment names,
full demand, authorised restart argv, and explicit `release_after_test` choice.
It preflights both sites, prepares/inspects/observes, invokes the authorised native
restart, reconnects/recovers and optionally releases. Failure writes private
partial-effect evidence instead of inventing rollback. It never migrates personal
Control or NOW. The example packet is documentation, not an installed configuration.

### Exact owner joins versus local proof

**WEB CODE joins:** Central #153's published effective-policy/NOW allocation result
must be projected into this material requirement (fresh refs/revision/expiry,
exact paths and required coverage). AIKit #275/#277 must use actual service
commands/state and attach the boundary to its real execution caller; #276 owns
recurrence semantics. Factory #221/#222 retains material/policy/coverage/usage
correlations and consumes actual observed results. No branch-private Central
schema or pretend gateway implementation is baked into Workcell.

**LOCAL PROOF:** approved exact-source installation, installed supervisor and
permissions, actual Central NOW allocation/adopted policy, native AIKit/harness
credentials/dispatch/reconnect/recurrence, required confinement on that host,
authorised source migration with byte preservation, independent physical second
placement, and independent whole-operation/human acceptance. A green controlled
campaign cannot close those cases or Workcell #72's full proving obligation.
