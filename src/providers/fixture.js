import { digest, RESULT_VERSION } from '../contracts.js';
import { invariant } from '../errors.js';

export class FixtureProvider {
  #worlds = new Map();
  constructor({ providerId = 'fixture', affordances = ['inspect', 'shell'], priority = 1, available = true, failRelease = false } = {}) {
    this.providerId = providerId;
    this.affordances = [...new Set(affordances)].sort();
    this.priority = priority;
    this.available = available;
    this.failRelease = failRelease;
  }
  offer() {
    return { providerId: this.providerId, available: this.available, affordances: [...this.affordances], priority: this.priority, offerRevision: digest({ providerId: this.providerId, variant: 'fixture', affordances: this.affordances, available: this.available }).slice(0, 16) };
  }
  prepare(demand, plan) {
    invariant(this.available, 'PROVIDER_UNAVAILABLE', 'fixture provider unavailable', { providerId: this.providerId });
    const key = digest({ providerId: this.providerId, variant: 'fixture', demand });
    const existing = this.#worlds.get(key);
    if (existing && existing.state === 'ready') return structuredClone(existing);
    const world = { worldRef: `world:${this.providerId}:${key.slice(0, 20)}`, bindingRef: `binding:${this.providerId}:${key.slice(0, 20)}`, providerId: this.providerId, state: 'ready', fixtureToken: `fixture-${key.slice(0, 8)}`, materialKey: key, preparedFromPlan: plan.planRef };
    this.#worlds.set(key, world);
    return structuredClone(world);
  }
  execute(binding, operation) {
    const world = this.#find(binding);
    invariant(world.state === 'ready', 'WORLD_NOT_READY', 'fixture world is not ready', { worldRef: world.worldRef });
    return { schemaVersion: RESULT_VERSION, status: 'ok', output: operation.kind === 'echo' ? operation.input ?? null : { state: world.state, fixtureToken: world.fixtureToken }, providerObservation: { providerId: this.providerId, fixtureToken: world.fixtureToken } };
  }
  inspect(binding) {
    const world = this.#find(binding);
    return { providerId: this.providerId, state: world.state, fixtureToken: world.fixtureToken };
  }
  release(binding) {
    const world = this.#find(binding);
    invariant(!this.failRelease, 'RELEASE_FAILED', 'fixture release failed', { bindingRef: binding.bindingRef });
    if (world.state === 'released') return { providerId: this.providerId, state: 'released', changed: false };
    world.state = 'released';
    this.#worlds.set(world.materialKey, world);
    return { providerId: this.providerId, state: 'released', changed: true };
  }
  #find(binding) {
    const world = [...this.#worlds.values()].find((candidate) => candidate.bindingRef === binding.bindingRef);
    invariant(world, 'BINDING_NOT_FOUND', 'fixture binding not found', { bindingRef: binding.bindingRef });
    return world;
  }
}
