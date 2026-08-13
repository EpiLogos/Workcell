# EpiLogos Workcell

Workcell is the **material execution product** in the EpiLogos software architecture.

It translates provider-neutral execution demands into concrete execution worlds while preserving the canonical semantic identities owned by Factory and other clients.

```text
semantic client
  -> ExecutionDemand
Workcell
  -> offers / eligibility / plan
  -> provider binding
  -> MaterialisedExecutionWorld
  -> execute / inspect / release
result + material provenance
  -> semantic owner
```

## Product boundary

Workcell is implemented here, in `EpiLogos/Workcell`. It is not miscellaneous Factory code.

The Factory repository `EpiLogos/agent-system-design` remains authoritative for the shared semantic architecture and root-contract programme. The detailed Workcell semantics are specified by its canonical Workcell module specification there. The repository-boundary clarification is tracked by Factory PR #126.

Core invariants:

- `ExecutionDemand` is provider-neutral.
- external semantic refs are opaque to Workcell and survive provider replacement.
- `CandidateMaterialisationDemand` is a constructor/view over `ExecutionDemand`.
- `MaterialisedExecutionWorld` and `Binding` are Workcell material concepts; Binding is not semantic identity.
- provider-specific data belongs in material bindings/provenance, not in shared semantic demand.
- Workcell correctness does not depend on QL/MEF availability.

## Current implementation

The initial product nucleus is dependency-free Node 22 ESM plus language-neutral JSON Schemas.

```js
import { Workcell, ReferenceProvider } from '@epilogos/workcell';

const workcell = new Workcell({ providers: [new ReferenceProvider()] });
const world = workcell.prepare({
  schemaVersion: 'workcell.execution-demand/v1',
  demandId: 'demo-1',
  subjectRef: 'candidate:demo',
  affordances: {
    required: ['shell', 'inspect'],
    preferred: ['snapshot'],
    optional: []
  }
});

const result = workcell.execute(world.worldRef, { kind: 'echo', input: 'hello' });
workcell.release(world.worldRef);
```

## Verification

One repository-owned operation is canonical:

```bash
npm run verify
```

It parses the public schemas, guards the provider-neutral demand schema against provider-specific vocabulary, and runs the complete Node test suite. GitHub Actions invokes the same command.

## Development programme

See Workcell issue #1. The first executable spans are #2 and #3. Factory cross-repository conformance remains gated by `EpiLogos/agent-system-design#113` and is not invented locally.
