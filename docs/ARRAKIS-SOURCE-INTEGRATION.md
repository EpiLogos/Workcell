# Arrakis provider source integration

**Ticket:** Workcell #13 / W12 / F.07  
**Factory source gate:** `EpiLogos/agent-system-design#24`  
**Integration mode:** first-party `arrakis-client` adapter over the upstream Arrakis REST API

## Pinned upstream

The provider is designed against:

- repository: `https://github.com/abshkbh/arrakis`
- source revision: `877231496acbf3b3091ab33340d2d126a251c4d5`
- OpenAPI document: `api/server-api.yaml`
- API document version: `2.0.0`
- license: GNU AGPL v3, with commercial licensing also described by upstream

This revision exposes the provider seam Workcell needs without reimplementing MicroVM management:

- list/create/destroy VMs;
- inspect one VM;
- pause/stop state operations;
- execute a command inside a VM;
- snapshot a VM;
- restore by starting a VM from `snapshotId`;
- upload/download files.

The upstream `arrakis-client` is a Go client of that REST API. The Workcell adapter invokes this first-party client and records the exact source/API revision in provider provenance. It does not invoke `cloud-hypervisor` or construct VM/tap/overlay lifecycle itself.

Primary source references:

- `https://github.com/abshkbh/arrakis/blob/877231496acbf3b3091ab33340d2d126a251c4d5/api/server-api.yaml`
- `https://github.com/abshkbh/arrakis/blob/877231496acbf3b3091ab33340d2d126a251c4d5/cmd/client/main.go`
- `https://github.com/abshkbh/arrakis/blob/877231496acbf3b3091ab33340d2d126a251c4d5/README.md`
- `https://github.com/abshkbh/arrakis/blob/877231496acbf3b3091ab33340d2d126a251c4d5/LICENSE`

## Provider boundary

The public semantic demand remains unchanged. A caller can ask for:

```text
required: shell
preferred: snapshot
isolation/trust: strong-isolation
```

and the planner may select any offer satisfying those requirements. The demand does not say `Arrakis`, `MicroVM`, `cloud-hypervisor`, `/dev/kvm`, a VM name, a tap device, or a snapshot directory.

Arrakis-specific material is returned only through the provider allocation/observation/operation provenance.

## Host topology and KVM

Upstream Arrakis currently requires Linux `/dev/kvm` on the machine running `arrakis-restserver`, because its current VMM is `cloud-hypervisor`.

The Workcell adapter therefore distinguishes two provider configurations:

1. **local-server configuration** — `require_local_kvm(true)`; discovery omits the Arrakis offer and prepare fails explicitly if `/dev/kvm` is not available to the Workcell host;
2. **remote-server configuration** — the client may run on a non-KVM client host while its configured `arrakis-restserver` runs on a suitable remote Linux/KVM host. Availability is then established by the first-party client reaching the server.

This distinction is provider configuration. It does not enter `ExecutionDemand`.

## Resource fidelity

The inspected `StartVMRequest` contains VM name, kernel, initramfs, rootfs, entry point and optional snapshot id. It does not expose per-VM CPU/memory sizing in this API revision.

The provider therefore rejects non-empty Workcell resource requests with `UnsatisfiedDemand` instead of silently pretending to satisfy them. A future Arrakis revision can add a new provider capability when its actual upstream API supports those semantics.

## Snapshot and restore

Snapshot/restore is a real provider capability, not a Candidate subtype.

`ArrakisExecutionProvider::snapshot_execution()` records:

```text
arrakis_vm_name
arrakis_snapshot_id
arrakis_source_revision
arrakis_api_version
```

`restore_execution()` records the exact restored snapshot id in provenance. The allocation's semantic/provider identity remains the same while the physical VM state is reconstructed through Arrakis.

`RetentionExpectation::SnapshotIfSupported` snapshots with a deterministic provider-local snapshot id and then destroys the live VM, returning the provider-neutral `Snapshotted` lifecycle disposition. Explicit snapshot operations are the evidence surface when the exact snapshot id must be retained by higher-level provenance.

## Restart recovery

The adapter does not require a process-local VM registry. It reconstructs the Arrakis VM name from persisted allocation properties/provenance and can inspect, act on, restore or release that material after the Workcell process restarts.

This aligns Arrakis with the same lifecycle/recovery contract used by the other providers.

## Verification

Deterministic tests cover:

- provider-port conformance;
- actual first-party client command mapping;
- shell and live-state observation;
- exact snapshot/restore provenance;
- local KVM absence;
- remote-client operation without local KVM;
- Arrakis service disappearance;
- unsupported resource sizing;
- provider-neutral planning parity with another strong-isolation provider;
- absence of Arrakis/MicroVM vocabulary from core semantic demand.

A real provider smoke is opt-in:

```bash
WORKCELL_ARRAKIS_LIVE=1 \
WORKCELL_ARRAKIS_CLIENT=/path/to/arrakis-client \
WORKCELL_ARRAKIS_CONFIG=/path/to/config.yaml \
./scripts/verify.sh
```

For a remote Arrakis server, set:

```bash
WORKCELL_ARRAKIS_REQUIRE_LOCAL_KVM=0
```

If the installed Arrakis setup needs explicit images, also set `WORKCELL_ARRAKIS_KERNEL` and `WORKCELL_ARRAKIS_ROOTFS`.

The source/host gate is not closed until this live smoke has been run against the pinned/recorded Arrakis installation on a compatible host and the observed source/configuration is recorded as evidence.
