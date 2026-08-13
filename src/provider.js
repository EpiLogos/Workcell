import { invariant } from './errors.js';

export function validateProvider(provider) {
  invariant(provider && typeof provider === 'object', 'INVALID_PROVIDER', 'provider must be an object');
  for (const method of ['offer', 'prepare', 'execute', 'inspect', 'release']) {
    invariant(typeof provider[method] === 'function', 'INVALID_PROVIDER', `provider must implement ${method}()`);
  }
  const offer = provider.offer();
  invariant(offer && typeof offer === 'object', 'INVALID_PROVIDER', 'provider offer must be an object');
  invariant(typeof offer.providerId === 'string' && offer.providerId.length > 0, 'INVALID_PROVIDER', 'providerId is required');
  invariant(typeof offer.available === 'boolean', 'INVALID_PROVIDER', 'provider availability must be boolean');
  invariant(Array.isArray(offer.affordances), 'INVALID_PROVIDER', 'provider affordances must be an array');
  return provider;
}

export function evaluateOffer(demand, offer) {
  const available = new Set(offer.affordances);
  const missingRequired = demand.affordances.required.filter((name) => !available.has(name));
  const missingPreferred = demand.affordances.preferred.filter((name) => !available.has(name));
  const missingOptional = demand.affordances.optional.filter((name) => !available.has(name));
  const matchedPreferred = demand.affordances.preferred.filter((name) => available.has(name));
  const matchedOptional = demand.affordances.optional.filter((name) => available.has(name));

  return {
    eligible: offer.available && missingRequired.length === 0,
    missingRequired,
    missingPreferred,
    missingOptional,
    score: matchedPreferred.length * 10 + matchedOptional.length + (offer.priority ?? 0)
  };
}
