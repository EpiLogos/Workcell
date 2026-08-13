# Workcell Architecture

## Responsibility

Workcell owns the material execution world.

It receives a provider-neutral `ExecutionDemand`, validates it, compares it with Workcell provider offers, produces a plan, binds one provider, returns a `MaterialisedExecutionWorld`, supports bounded operations against that world, and releases or reconciles material state.

Workcell does not own the semantic meaning of the subject being materialised.

## Public seam

```text
ExecutionDemand
  -> validate
  -> discover offers
  -> plan
  -> prepare
  -> MaterialisedExecutionWorld
      -> Binding[]
      -> material provenance
  -> execute / inspect
  -> release
```

### ExecutionDemand

The v1 demand contains only material requirements and opaque semantic references. It has no provider selector. Required/preferred/optional affordances are distinct so degradation is explicit.

### Planning

`plan()` is not binding. It records provider eligibility, the selected provider and any degradation, but it creates no material resource.

### MaterialisedExecutionWorld

A world is a Workcell-owned material result. It carries the caller's `subjectRef` unchanged and records Workcell/provider provenance separately.

### Binding

A Binding is the current material resolution of a logical resource. It may include provider-specific details. It is deliberately distinct from the external semantic ref.

## Provider contract

A provider implements:

```text
offer()
prepare(demand, plan)
execute(binding, operation)
inspect(binding)
release(binding)
```

The initial reference provider uses in-memory material state. A second process-shaped fixture uses a different binding/provenance form to prove that public semantics are not coupled to one provider representation.

## Identity rules

```text
subjectRef        semantic owner; Workcell preserves
worldRef          Workcell material result identity
bindingRef        ephemeral/current provider resolution
providerId        Workcell-local provider identity
providerDetails   provider-specific material information
```

Changing `worldRef`, `bindingRef`, provider identity or provider details must not mutate `subjectRef`.

## Failure and degradation

- no provider satisfying required affordances -> `unsatisfiable` plan / prepare failure;
- preferred/optional gaps -> explicit degraded plan;
- unavailable provider -> ineligible for binding;
- operation against unavailable/released material state -> structured failure;
- cleanup failure -> structured `RELEASE_FAILED`, never silent success.

## Cross-repository seam

Factory #11 and #113 are still evolving. V1 therefore treats external semantic refs as opaque and does not declare Factory `Project`, `Candidate`, `Run` or `Execution` object schemas.

When the applicable #113 fixture subset is stable, Workcell issue #6 will import and validate it. That later conformance step must not change the provider-neutral design established here.
