# Factory / Workcell interop

**Ticket:** Workcell #14 / W13  
**Factory dependency:** `EpiLogos/agent-system-design#113`  
**Consumed fixture:** `factory.interop-fixtures/v1`  
**Consumed protocol:** `factory.interop/v1`  
**Pinned Factory source revision:** `474a4c2c13854a5ea253d77f5aff4aa491ced2c5`

## Boundary

Cross-repository conformance lives in the separate `epilogos-workcell-interop` crate. `epilogos-workcell-core` does not depend on Factory code, Factory fixture packages, JSON libraries, or Factory semantic types.

The adapter consumes a language-neutral JSON fixture copied from the #113 closure surface and validates the Workcell-relevant slice:

```text
executionDemand
workcellOffer
binding
identity anti-fixtures
```

The raw JSON value is retained alongside the adapted view so a shared fixture can be parsed and serialized without dropping non-Workcell fields. This is the cross-language round-trip surface; Workcell does not pretend to own the rest of the Factory contract.

## Opaque identity

Factory-specific semantic identity encoding is checked only in the interop adapter:

```text
Project   factory:project:...
Run       factory:run:...
Candidate factory:candidate:...
Agent     factory:agent:...
```

After validation, those values enter Workcell as opaque `ExternalRef`s carried in `ExecutionDemand.subjects`.

Provider/worktree/container/VM/process identifiers fail the semantic-role validator. This prevents the shared boundary from turning material identity into Project, Run, Candidate or Agent identity while keeping Factory meaning out of `workcell-core`.

## ExecutionDemand adaptation

The stable v1 Factory fixture maps without provider selection:

- project/run/candidate refs -> opaque semantic subjects;
- required/preferred/optional affordances -> Workcell necessity tiers;
- `cpu>=2` / `memory>=4GiB` -> provider-neutral resource requirements;
- connectivity -> required logical connections;
- browser exposure -> required exposure;
- writable project checkout -> writable workspace semantics;
- run + candidate persistence -> the strongest world-level persistence needed to satisfy both (`Candidate`).

Some Factory v1 fields are not isomorphic to current Workcell fields. The adapter preserves those values under namespaced extensions instead of silently changing their strength or meaning. In particular, Factory `strong-preferred` is not promoted into Workcell's current non-tiered `isolation_trust` field; the shared preferred `isolated-execution` affordance remains the operative preference.

Provider substitution is tested by changing the shared binding from one provider/concrete resource to another and asserting that the resulting semantic `ExecutionDemand` is byte-for-byte equal as a Rust value.

## Version failure

Both version boundaries are explicit:

```text
factory.interop-fixtures/v1
factory.interop/v1
```

Any other fixture or protocol version returns `WorkcellError::Unsupported`. There is no permissive fallback or guess at future schema meaning.

## Consumed-version evidence

`InteropConsumptionReport` records:

- fixture version;
- protocol version;
- pinned Factory source revision and fixture path;
- Workcell ref;
- binding logical/provider/concrete refs;
- opaque semantic project/run/candidate subjects.

This makes the exact interoperability surface visible in test/run evidence.

## Standalone core proof

The interop conformance test explicitly checks that `crates/workcell-core/Cargo.toml` contains no Factory/interop/JSON dependency and that core demand source contains no Factory identity prefixes.

The ordinary independent core command remains:

```bash
cargo test -p epilogos-workcell-core --all-targets
```

The full workspace additionally runs the copied cross-repository fixture tests. No checkout, package import or runtime connection to the Factory repository is needed to build or test Workcell core.
