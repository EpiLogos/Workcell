---
role: architecture
standing: agent-inference
scope: Workcell native operations and composed O:I consumer boundaries
updated: 2026-09-30
---
# Workcell architecture navigation

This is an implementation-facing navigation companion for O:I #65/#220 and the
existing documentation programme. It recovers native owners and successors;
it does not adopt a new design or claim the whole running experience complete.
Inspected native checkout revision: `8d32e20ba4158f3ac11ab36d8a5b2d6c9c177af0`. Active repair source may advance
that cut; identify the file revision before relying on its returned result.

## Governing source and successors

- [ARCHITECTURE](ARCHITECTURE.md)
- [CONTROL-SERVICE-AND-AGENT-HOSTING](CONTROL-SERVICE-AND-AGENT-HOSTING.md)
- [CONNECTIVITY-FABRIC](CONNECTIVITY-FABRIC.md)


Directory names are routes, not authority. Target design, amendment, historical
baseline, current implementation and observed result keep their own standing.

| Concern | Public operation / entry | Native source | Boundary and lifecycle |
| --- | --- | --- | --- |
| Material operations | Demand → plan/place → observe → collect → release | `crates/workcell-core/src/prepared.rs`; `crates/workcell-runtime/src/run.rs` | Native material Run may carry an optional Factory canonical_run_ref; Factory-less material Run is first class. |
| Durable material Run | RunLedger record / CAS update | `crates/workcell-runtime/src/run.rs` | runs/<slug>.json authoritative; runs.json derived; shared lock prevents observation overwriting release. |
| Service instance lifecycle | Native service instances / reconciliation | `crates/workcell-runtime/src/instance_registry.rs`; `service.rs` | Instance identity/generation and service storage differ from AIKit session or gateway owner PID. |
| Control / connectivity | Authenticated Control Service / provider fabric | `crates/workcell-control/src/service.rs`; `crates/workcell-runtime/src/external_service.rs` | Transport and credentials are material carriers, not the purpose or canonical participant identity. |

## Diagram and consumer relation

The maintained suite companion is
`source:project:O-I:docs/architecture/upgrade-lifecycle.md`; its editable diagram is
`source:project:O-I:docs/architecture/upgrade-lifecycle.mmd`. The O:I architecture entry
contains six question-specific companions, full-size rendered SVGs, an indexed
basis for every arrow, exact source hashes and independent navigation evidence.
Resolve that source in the current O:I checkout before substituting a cached or
historical copy. Its solid arrows are inspected relations, not installed
acceptance; proposed joins remain explicitly proposed.

Existing capability records link this companion through the optional
`extensions.documentation` protocol. These links change discoverability, not
capability IDs, coordinate placements, implementation status or source authority.

## Verification and open joins

Read each native test and its actual runner conditions, then the corresponding
dated Return. A test definition is not an executed result; a process/receipt is
not human Recognition. The suite's architecture verification records real
Mermaid rendering, source/link checks and the fresh-agent navigation task.
The four repair lanes continue to own their code, installed replay and open
architectural decisions. Preserve a missing join as missing until that proof
or decision is returned.
