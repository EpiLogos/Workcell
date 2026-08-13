import {
  PLAN_VERSION,
  WORLD_VERSION,
  asWorkcellError,
  assertOperation,
  digest,
  validateExecutionDemand
} from './contracts.js';
import { WorkcellError, invariant } from './errors.js';
import { evaluateOffer, validateProvider } from './provider.js';

export class Workcell {
  #providers = new Map();
  #worlds = new Map();

  constructor({ workcellId = 'workcell:local', providers = [] } = {}) {
    invariant(typeof workcellId === 'string' && workcellId.length > 0, 'INVALID_WORKCELL', 'workcellId is required');
    this.workcellId = workcellId;
    for (const provider of providers) this.register(provider);
  }

  register(provider) {
    validateProvider(provider);
    const providerId = provider.offer().providerId;
    invariant(!this.#providers.has(providerId), 'DUPLICATE_PROVIDER', `provider ${providerId} is already registered`, { providerId });
    this.#providers.set(providerId, provider);
    return this;
  }

  discover() {
    const offers = [...this.#providers.values()].map((provider) => provider.offer()).sort((a, b) => a.providerId.localeCompare(b.providerId));
    return Object.freeze({ workcellId: this.workcellId, offers });
  }

  plan(input) {
    const demand = validateExecutionDemand(input);
    const evaluated = [...this.#providers.values()].map((provider) => {
      const offer = provider.offer();
      return { offer, assessment: evaluateOffer(demand, offer) };
    });

    const eligible = evaluated
      .filter(({ assessment }) => assessment.eligible)
      .sort((a, b) => b.assessment.score - a.assessment.score || a.offer.providerId.localeCompare(b.offer.providerId));

    if (eligible.length === 0) {
      return Object.freeze({
        schemaVersion: PLAN_VERSION,
        planRef: `plan:${digest({ workcellId: this.workcellId, demand, status: 'unsatisfiable' }).slice(0, 24)}`,
        workcellId: this.workcellId,
        demandId: demand.demandId,
        subjectRef: demand.subjectRef,
        status: 'unsatisfiable',
        selectedProviderId: null,
        degradations: [],
        candidates: evaluated.map(({ offer, assessment }) => ({
          providerId: offer.providerId,
          available: offer.available,
          eligible: false,
          missingRequired: assessment.missingRequired
        }))
      });
    }

    const selected = eligible[0];
    const degradations = [
      ...selected.assessment.missingPreferred.map((affordance) => ({ affordance, necessity: 'preferred' })),
      ...selected.assessment.missingOptional.map((affordance) => ({ affordance, necessity: 'optional' }))
    ];
    const planRef = `plan:${digest({ workcellId: this.workcellId, demand, providerId: selected.offer.providerId, offerRevision: selected.offer.offerRevision }).slice(0, 24)}`;

    return Object.freeze({
      schemaVersion: PLAN_VERSION,
      planRef,
      workcellId: this.workcellId,
      demandId: demand.demandId,
      subjectRef: demand.subjectRef,
      status: degradations.length > 0 ? 'degraded' : 'satisfiable',
      selectedProviderId: selected.offer.providerId,
      offerRevision: selected.offer.offerRevision,
      degradations,
      candidates: evaluated.map(({ offer, assessment }) => ({
        providerId: offer.providerId,
        available: offer.available,
        eligible: assessment.eligible,
        missingRequired: assessment.missingRequired,
        score: assessment.score
      }))
    });
  }

  prepare(input) {
    const demand = validateExecutionDemand(input);
    const plan = this.plan(demand);
    invariant(plan.selectedProviderId, 'UNSATISFIABLE_DEMAND', 'no registered provider can satisfy required affordances', { plan });
    const provider = this.#providers.get(plan.selectedProviderId);

    let material;
    try {
      material = provider.prepare(demand, plan);
    } catch (error) {
      throw asWorkcellError(error);
    }

    const binding = Object.freeze({
      bindingRef: material.bindingRef,
      logicalRef: 'execution:self',
      providerId: material.providerId,
      state: material.state,
      providerDetails: Object.freeze(Object.fromEntries(
        Object.entries(material).filter(([key]) => !['worldRef', 'bindingRef', 'providerId', 'state', 'materialKey', 'preparedFromPlan'].includes(key))
      ))
    });

    const world = Object.freeze({
      schemaVersion: WORLD_VERSION,
      worldRef: material.worldRef,
      workcellId: this.workcellId,
      demandId: demand.demandId,
      subjectRef: demand.subjectRef,
      ...(demand.subjectRevision ? { subjectRevision: demand.subjectRevision } : {}),
      state: material.state,
      planRef: plan.planRef,
      provider: Object.freeze({ id: material.providerId, offerRevision: plan.offerRevision }),
      bindings: Object.freeze([binding]),
      degradations: Object.freeze(plan.degradations.map((item) => Object.freeze({ ...item }))),
      provenance: Object.freeze({
        demandDigest: digest(demand),
        providerId: material.providerId,
        materialKey: material.materialKey,
        preparedFromPlan: material.preparedFromPlan
      })
    });

    this.#worlds.set(world.worldRef, { world, demand, providerId: material.providerId, binding });
    return world;
  }

  inspect(worldRef) {
    const record = this.#requireWorld(worldRef);
    const provider = this.#providers.get(record.providerId);
    const observation = provider.inspect(record.binding);
    return Object.freeze({
      worldRef,
      subjectRef: record.world.subjectRef,
      provider: Object.freeze({ id: record.providerId }),
      observation: Object.freeze({ ...observation })
    });
  }

  execute(worldRef, inputOperation) {
    const record = this.#requireWorld(worldRef);
    const operation = assertOperation(inputOperation);
    const provider = this.#providers.get(record.providerId);
    try {
      const result = provider.execute(record.binding, operation);
      return Object.freeze({
        ...result,
        worldRef,
        subjectRef: record.world.subjectRef,
        operation: operation.kind
      });
    } catch (error) {
      throw asWorkcellError(error);
    }
  }

  release(worldRef) {
    const record = this.#requireWorld(worldRef);
    const provider = this.#providers.get(record.providerId);
    try {
      const released = provider.release(record.binding);
      return Object.freeze({
        worldRef,
        subjectRef: record.world.subjectRef,
        provider: Object.freeze({ id: record.providerId }),
        state: released.state,
        changed: released.changed
      });
    } catch (error) {
      const workcellError = asWorkcellError(error, 'RELEASE_FAILED');
      throw new WorkcellError(workcellError.code, workcellError.message, {
        ...workcellError.details,
        worldRef,
        subjectRef: record.world.subjectRef,
        providerId: record.providerId
      });
    }
  }

  #requireWorld(worldRef) {
    invariant(typeof worldRef === 'string' && worldRef.length > 0, 'WORLD_NOT_FOUND', 'worldRef is required');
    const record = this.#worlds.get(worldRef);
    invariant(record, 'WORLD_NOT_FOUND', 'materialised world is not known to this Workcell', { worldRef });
    return record;
  }
}
