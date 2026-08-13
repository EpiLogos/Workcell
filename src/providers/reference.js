import { digest, RESULT_VERSION } from '../contracts.js';
import { invariant } from '../errors.js';

export class ReferenceProvider {
  #worlds = new Map();

  constructor({ providerId = 'reference', affordances = ['inspect', 'shell', 'snapshot'], priority = 10, available = true } = {}) {
    this.providerId = providerId;
    this.affordances = [...new Set(affordances)].sort();
    this.priority = priority;
    this.available = available;
  }

  offer() {
    return Object.freeze({
      providerId: this.providerId,
      available: this.available,
      affordances: [...this.affordances],
      priority: this.priority,
      offerRevision: digest({ providerId: this.providerId, available: this.available, affordances: this.affordances, priority: this.priority }).slice(0, 16)
    });
  }

  prepare(demand, plan) {
    invariant(this.available, 'PROVIDER_UNAVAILABLE', `${this.providerId} is unavailable`, { providerId: this.providerId });
    const materialKey = digest({ providerId: this.providerId, demand });
    const existing = this.#worlds.get(materialKey);
    if (existing && existing.state === 'ready') return structuredClone(existing);

    const bindingRef = `binding:${this.providerId}:${materialKey.slice(0, 20)}`;
    const worldRef = `world:${this.providerId}:${materialKey.slice(0, 20)}`;
    const world = {
      worldRef,
      bindingRef,
      providerId: this.providerId,
      state: 'ready',
      address: `memory://${this.providerId}/${materialKey.slice(0, 20)}`,
      materialKey,
      preparedFromPlan: plan.planRef
    };
    this.#worlds.set(materialKey, world);
    return structuredClone(world);
  }

  execute(binding, operation) {
    const world = this.#find(binding);
    invariant(world.state === 'ready', 'WORLD_NOT_READY', 'materialised world is not ready', { worldRef: world.worldRef, state: world.state });
    if (operation.kind === 'echo') {
      return {
        schemaVersion: RESULT_VERSION,
        status: 'ok',
        output: operation.input ?? null,
        providerObservation: { providerId: this.providerId, worldState: world.state }
      };
    }
    return {
      schemaVersion: RESULT_VERSION,
      status: 'ok',
      output: { state: world.state, address: world.address },
      providerObservation: { providerId: this.providerId, worldState: world.state }
    };
  }

  inspect(binding) {
    const world = this.#find(binding);
    return { providerId: this.providerId, state: world.state, address: world.address };
  }

  release(binding) {
    const world = this.#find(binding);
    if (world.state === 'released') return { providerId: this.providerId, state: 'released', changed: false };
    world.state = 'released';
    this.#worlds.set(world.materialKey, world);
    return { providerId: this.providerId, state: 'released', changed: true };
  }

  #find(binding) {
    const world = [...this.#worlds.values()].find((candidate) => candidate.bindingRef === binding.bindingRef);
    invariant(world, 'BINDING_NOT_FOUND', 'binding is not known to provider', { bindingRef: binding.bindingRef });
    return world;
  }
}
