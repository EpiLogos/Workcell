# Candidate materialisation and lifecycle

**Ticket:** Workcell #15 / W14 / F.10

## Ownership

Candidate is a Factory semantic identity. Workcell does not create Candidate identities, assign Candidate revisions, compare Candidate equivalence, or decide that a material change constitutes a new Candidate.

`epilogos-workcell-candidate` therefore contains only `CandidateMaterialisationDemand`, a thin view over the existing provider-neutral `ExecutionDemand`. It requires one externally supplied opaque `candidate` subject and preserves that reference across repeated materialisation.

## Materialisation

The same Candidate may be materialised more than once with different:

- Workcell identities;
- provider selections;
- workspace/runtime resources;
- physical material references;
- observed health;
- application/browser endpoints;
- provenance revisions.

Those changes belong to material state. They do not rewrite the external Candidate reference.

The integration wrapper has no provider field. Placement/provider choice remains the job of Workcell discovery/planning.

## Release and failure

A live material world can be observed and released through the ordinary `PreparedWorldControlPlane`. Successful release updates binding lifecycle state to `Released` while retaining semantic subjects, including Candidate.

A later materialisation may then bind the same Candidate reference to a different Workcell/provider/material world. Workcell reports the material difference; it does not infer a new Candidate revision.

Likewise, provider disappearance or withdrawn offers change material availability and reconciliation state (`Stale`, `Missing`, `recover`, `rematerialise`, and related lifecycle deltas). The Candidate subject remains unchanged so the semantic owner can decide what the material event means for Candidate evidence or revisioning.

## Exposure rebind

Application/browser exposure is derived from current runtime material. A changed physical endpoint or exposure provider does not change Candidate identity. Exposure provenance remains material evidence associated with the same externally supplied semantic subject.

## Conformance

`candidate_materialisation.rs` proves:

- the wrapper is only a view over `ExecutionDemand`;
- conflicting candidate subjects are rejected;
- the same Candidate survives Workcell/provider/material/exposure rebinding;
- provider observation returns structured material evidence;
- release changes binding presence but preserves Candidate identity;
- rematerialisation changes material identity but preserves Candidate identity;
- provider loss produces unavailable observation and a recovery delta without changing Candidate identity;
- the integration crate contains no Candidate revision, new-Candidate, equivalence, or provider-selection authority.

This preserves the constitutional boundary: Workcell materialises possible runtime worlds beneath Candidate; it does not own the Candidate ontology itself.
