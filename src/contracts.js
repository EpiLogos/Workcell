import { createHash } from 'node:crypto';
import { WorkcellError, invariant } from './errors.js';

export const EXECUTION_DEMAND_VERSION = 'workcell.execution-demand/v1';
export const PLAN_VERSION = 'workcell.plan/v1';
export const WORLD_VERSION = 'workcell.materialised-execution-world/v1';
export const RESULT_VERSION = 'workcell.operation-result/v1';

export const NECESSITIES = Object.freeze(['required', 'preferred', 'optional']);
export const PERSISTENCE_SCOPES = Object.freeze([
  'ephemeral',
  'run',
  'candidate',
  'project',
  'workcell',
  'factory',
  'external'
]);

function assertPlainObject(value, field) {
  invariant(value !== null && typeof value === 'object' && !Array.isArray(value), 'INVALID_DEMAND', `${field} must be an object`, { field });
}

function assertString(value, field) {
  invariant(typeof value === 'string' && value.trim().length > 0, 'INVALID_DEMAND', `${field} must be a non-empty string`, { field });
}

function assertOnlyKeys(object, allowed, field) {
  const extras = Object.keys(object).filter((key) => !allowed.includes(key));
  invariant(extras.length === 0, 'INVALID_DEMAND', `${field} contains unsupported fields`, { field, extras });
}

function normalizeAffordances(value = {}) {
  assertPlainObject(value, 'affordances');
  assertOnlyKeys(value, NECESSITIES, 'affordances');
  const seen = new Map();
  const normalized = {};

  for (const necessity of NECESSITIES) {
    const list = value[necessity] ?? [];
    invariant(Array.isArray(list), 'INVALID_DEMAND', `affordances.${necessity} must be an array`, { necessity });
    normalized[necessity] = list.map((name, index) => {
      assertString(name, `affordances.${necessity}[${index}]`);
      const clean = name.trim();
      invariant(!seen.has(clean), 'INVALID_DEMAND', `affordance ${clean} is declared more than once`, {
        affordance: clean,
        first: seen.get(clean),
        second: necessity
      });
      seen.set(clean, necessity);
      return clean;
    }).sort();
  }

  return normalized;
}

function normalizeWorkspace(value) {
  if (value === undefined) return undefined;
  assertPlainObject(value, 'workspace');
  assertOnlyKeys(value, ['writable', 'sourceRef', 'sourceRevision'], 'workspace');
  invariant(typeof value.writable === 'boolean', 'INVALID_DEMAND', 'workspace.writable must be boolean', {});
  if (value.sourceRef !== undefined) assertString(value.sourceRef, 'workspace.sourceRef');
  if (value.sourceRevision !== undefined) assertString(value.sourceRevision, 'workspace.sourceRevision');
  return {
    writable: value.writable,
    ...(value.sourceRef ? { sourceRef: value.sourceRef.trim() } : {}),
    ...(value.sourceRevision ? { sourceRevision: value.sourceRevision.trim() } : {})
  };
}

export function validateExecutionDemand(input) {
  assertPlainObject(input, 'ExecutionDemand');
  assertOnlyKeys(input, [
    'schemaVersion',
    'demandId',
    'subjectRef',
    'subjectRevision',
    'affordances',
    'connect',
    'workspace',
    'persistence',
    'retention'
  ], 'ExecutionDemand');

  invariant(input.schemaVersion === EXECUTION_DEMAND_VERSION, 'UNSUPPORTED_DEMAND_VERSION', `schemaVersion must be ${EXECUTION_DEMAND_VERSION}`, {
    received: input.schemaVersion
  });
  assertString(input.demandId, 'demandId');
  assertString(input.subjectRef, 'subjectRef');
  if (input.subjectRevision !== undefined) assertString(input.subjectRevision, 'subjectRevision');

  const connect = input.connect ?? [];
  invariant(Array.isArray(connect), 'INVALID_DEMAND', 'connect must be an array', {});
  const normalizedConnect = [...new Set(connect.map((entry, index) => {
    assertString(entry, `connect[${index}]`);
    return entry.trim();
  }))].sort();

  const persistence = input.persistence ?? 'ephemeral';
  invariant(PERSISTENCE_SCOPES.includes(persistence), 'INVALID_DEMAND', 'persistence has an unsupported scope', { persistence });

  const retention = input.retention ?? 'release';
  invariant(retention === 'release' || retention === 'preserve', 'INVALID_DEMAND', 'retention must be release or preserve', { retention });

  return Object.freeze({
    schemaVersion: EXECUTION_DEMAND_VERSION,
    demandId: input.demandId.trim(),
    subjectRef: input.subjectRef.trim(),
    ...(input.subjectRevision ? { subjectRevision: input.subjectRevision.trim() } : {}),
    affordances: normalizeAffordances(input.affordances),
    connect: normalizedConnect,
    ...(input.workspace ? { workspace: normalizeWorkspace(input.workspace) } : {}),
    persistence,
    retention
  });
}

export function candidateMaterialisationDemand(candidateRef, demand) {
  assertString(candidateRef, 'candidateRef');
  return validateExecutionDemand({
    ...demand,
    schemaVersion: EXECUTION_DEMAND_VERSION,
    subjectRef: candidateRef.trim()
  });
}

export function stableJson(value) {
  if (Array.isArray(value)) return `[${value.map(stableJson).join(',')}]`;
  if (value && typeof value === 'object') {
    const entries = Object.entries(value)
      .filter(([, item]) => item !== undefined)
      .sort(([a], [b]) => a.localeCompare(b));
    return `{${entries.map(([key, item]) => `${JSON.stringify(key)}:${stableJson(item)}`).join(',')}}`;
  }
  return JSON.stringify(value);
}

export function digest(value) {
  return createHash('sha256').update(stableJson(value)).digest('hex');
}

export function assertOperation(operation) {
  assertPlainObject(operation, 'operation');
  assertOnlyKeys(operation, ['kind', 'input'], 'operation');
  invariant(operation.kind === 'echo' || operation.kind === 'inspect', 'UNSUPPORTED_OPERATION', 'operation.kind must be echo or inspect', {
    kind: operation.kind
  });
  return Object.freeze({ kind: operation.kind, ...(operation.input !== undefined ? { input: operation.input } : {}) });
}

export function asWorkcellError(error, fallbackCode = 'PROVIDER_FAILURE') {
  if (error instanceof WorkcellError) return error;
  return new WorkcellError(fallbackCode, error instanceof Error ? error.message : String(error), {});
}
