# Multi-Workcell placement and remote availability

**Ticket:** Workcell #16 / W15 / F.13

## Boundary

A remote Workcell exposes the same canonical public contract as a local Workcell. Placement therefore consumes `Discovery` values and runs the same provider-neutral `plan()` operation regardless of where the operational domain physically lives.

`epilogos-workcell-placement` introduces no remote execution ontology into core. It defines only a replaceable `WorkcellDiscoverySource` seam that can supply canonical `Discovery` plus opaque placement metadata:

- source identity;
- optional Workcell identity hint;
- locality cost;
- policy tags;
- transport provenance.

How that source reaches the Workcell is deliberately outside the contract. In-process, remote, authenticated, encrypted, event-driven, or later transport implementations can replace each other without changing `ExecutionDemand`.

## Placement

Placement evaluates each reachable Workcell by:

1. explicit policy limits/tags;
2. Workcell discovery health;
3. Workcell-wide aggregate capacity where declared/required;
4. the ordinary core materialisation plan and its necessity semantics;
5. locality cost and capacity headroom according to placement policy.

The result is a `PlacementDecision` containing the selected `WorkcellRef`, the ordinary `MaterialisationPlan`, and placement provenance. It does not mutate the semantic demand.

The policy can require declared aggregate capacity, cap locality cost, require opaque placement tags, and decide whether locality sorts ahead of spare capacity. Provider-specific resource matching remains the core planner's job; aggregate capacity is an additional whole-Workcell placement signal, not a replacement for provider offers.

## Failure and diagnostics

The placement layer distinguishes:

- `TransportUnavailable` — the discovery source cannot currently reach the Workcell;
- `WorkcellUnavailable` — discovery succeeded but the Workcell reports unavailable health;
- `PolicyRejected` — locality or policy-tag constraints reject the placement;
- `CapacityRejected` — required whole-Workcell capacity is absent/incompatible/insufficient;
- `Unsatisfiable` — the normal Workcell plan cannot satisfy the demand.

These are material-placement facts. They do not mutate semantic Project, Run, Candidate or Agent refs carried in the demand.

## Re-placement and provenance

A later placement can be evaluated with the prior `WorkcellRef`. The resulting provenance records:

- current Workcell;
- previous Workcell;
- whether placement changed;
- selected provider refs and offer refs;
- capacity headroom;
- locality cost;
- opaque transport/source provenance.

This is enough to retain cross-Workcell material provenance while allowing provider substitution. The semantic client continues to hold the same demand and external subjects.

## Conformance

`multi_workcell.rs` proves:

- capacity-driven choice between multiple satisfying Workcells;
- explicit locality/policy preference without hostnames;
- remote transport loss as a structured diagnostic;
- reachable-but-unavailable Workcell as a distinct diagnostic;
- re-placement after loss with different Workcell/provider and retained previous-placement provenance;
- Project/Run/Candidate/Agent subject maps remain unchanged across placement and re-placement;
- the production placement seam contains no fixed cluster, host, credential, or concrete transport configuration.

This remains intentionally below the Factory's semantic ownership layer and above individual provider adapters: Workcell placement decides **where an unchanged material demand can be satisfied**, not what the software-development subject means.

## Secret origin and projection

Placement moves an unchanged material demand between cells. It never moves
the secret store. A credential is stored once, on the machine where the
person put it — the origin cell's secret source (macOS Keychain,
1Password, or the Linux Secret Service, all under the same
`SecretProvider` contract). What follows placement is projection, not
replication:

- the origin cell holds a `SecretProjectionRequest` (source credential ref,
  target workcell/sandbox relation, the one authorised materialisation
  class, purpose, scope) in `<state-root>/secrets/projections.json`;
- the target receives an authorised reference/materialisation relation.
  Raw material crosses only the existing trusted sink boundaries — for a
  sandbox, the OpenSandbox Credential Vault broker; for another workcell, a
  materialisation presented back to the origin at use time over an
  authorised cross-cell connection;
- receipts carry refs, never values: a `SecretProjectionReceipt` embeds the
  ordinary `SecretMaterialReceipt` and adds no material;
- revocation at the origin reaches the projected target at its next use —
  a revoked projection refuses before any materialisation and before any
  provider-side write;
- `workcell secret scan` sees credential material that never entered a
  provider (environment variables, shell rc files, known auth files) and
  reports location + presence only; `workcell secret vault` moves that
  material into the origin store without the value ever being printed;
  detection failure is a declared refusal, never a silent skip.

No second store, no per-machine copies, no split state: a cell that cannot
reach its origin provider materialises nothing rather than falling back to
plaintext, and says so — "could not run" never collapses into "ran and
found nothing".
