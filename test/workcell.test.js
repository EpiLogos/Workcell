import test from 'node:test';
import assert from 'node:assert/strict';
import {
  EXECUTION_DEMAND_VERSION,
  ReferenceProvider,
  Workcell,
  WorkcellError,
  candidateMaterialisationDemand,
  validateExecutionDemand
} from '../src/index.js';

function demand(overrides = {}) {
  return {
    schemaVersion: EXECUTION_DEMAND_VERSION,
    demandId: 'demand-001',
    subjectRef: 'candidate:alpha',
    affordances: {
      required: ['shell', 'inspect'],
      preferred: ['snapshot'],
      optional: ['browser']
    },
    connect: ['internet', 'project:self'],
    persistence: 'candidate',
    retention: 'release',
    ...overrides
  };
}

test('validates and normalizes a provider-neutral demand', () => {
  const value = validateExecutionDemand(demand());
  assert.equal(value.subjectRef, 'candidate:alpha');
  assert.deepEqual(value.affordances.required, ['inspect', 'shell']);
  assert.deepEqual(value.connect, ['internet', 'project:self']);
});

test('rejects provider selector fields and duplicate affordances', () => {
  assert.throws(
    () => validateExecutionDemand({ ...demand(), provider: 'reference' }),
    (error) => error instanceof WorkcellError && error.code === 'INVALID_DEMAND'
  );
  assert.throws(
    () => validateExecutionDemand({
      ...demand(),
      affordances: { required: ['shell'], preferred: ['shell'], optional: [] }
    }),
    (error) => error instanceof WorkcellError && error.code === 'INVALID_DEMAND'
  );
});

test('CandidateMaterialisationDemand is only a constructor over ExecutionDemand', () => {
  const base = { ...demand() };
  delete base.subjectRef;
  const value = candidateMaterialisationDemand('candidate:stable', base);
  assert.equal(value.schemaVersion, EXECUTION_DEMAND_VERSION);
  assert.equal(value.subjectRef, 'candidate:stable');
});

test('planning separates eligibility from binding and reports degradation', () => {
  const workcell = new Workcell({ providers: [
    new ReferenceProvider({ affordances: ['inspect', 'shell', 'snapshot'] })
  ] });
  const plan = workcell.plan(demand());
  assert.equal(plan.selectedProviderId, 'reference');
  assert.equal(plan.status, 'degraded');
  assert.deepEqual(plan.degradations, [{ affordance: 'browser', necessity: 'optional' }]);
  assert.equal('bindings' in plan, false);
  assert.equal('bindingRef' in plan, false);
});

test('required affordances cannot be silently dropped', () => {
  const workcell = new Workcell({ providers: [new ReferenceProvider()] });
  const plan = workcell.plan(demand({
    affordances: { required: ['shell', 'gpu'], preferred: [], optional: [] }
  }));
  assert.equal(plan.status, 'unsatisfiable');
  assert.equal(plan.selectedProviderId, null);
  assert.deepEqual(plan.candidates[0].missingRequired, ['gpu']);
  assert.throws(
    () => workcell.prepare(demand({ affordances: { required: ['gpu'], preferred: [], optional: [] } })),
    (error) => error instanceof WorkcellError && error.code === 'UNSATISFIABLE_DEMAND'
  );
});

test('tracer bullet materialises, executes, inspects and releases structurally', () => {
  const workcell = new Workcell({ workcellId: 'workcell:test', providers: [new ReferenceProvider()] });
  const world = workcell.prepare(demand({ affordances: { required: ['shell', 'inspect'], preferred: ['snapshot'], optional: [] } }));
  assert.equal(world.subjectRef, 'candidate:alpha');
  assert.equal(world.provider.id, 'reference');
  assert.match(world.bindings[0].bindingRef, /^binding:reference:/);
  assert.equal(world.bindings[0].logicalRef, 'execution:self');

  const result = workcell.execute(world.worldRef, { kind: 'echo', input: { hello: 'world' } });
  assert.equal(result.status, 'ok');
  assert.deepEqual(result.output, { hello: 'world' });
  assert.equal(result.subjectRef, 'candidate:alpha');

  const observation = workcell.inspect(world.worldRef);
  assert.equal(observation.subjectRef, 'candidate:alpha');
  assert.equal(observation.observation.state, 'ready');

  const released = workcell.release(world.worldRef);
  assert.equal(released.state, 'released');
  assert.equal(released.changed, true);
  const releasedAgain = workcell.release(world.worldRef);
  assert.equal(releasedAgain.changed, false);
});

test('prepare is idempotent while a deterministic world remains ready', () => {
  const workcell = new Workcell({ providers: [new ReferenceProvider()] });
  const d = demand({ affordances: { required: ['shell'], preferred: [], optional: [] } });
  const first = workcell.prepare(d);
  const second = workcell.prepare(d);
  assert.equal(first.worldRef, second.worldRef);
  assert.equal(first.bindings[0].bindingRef, second.bindings[0].bindingRef);
});
