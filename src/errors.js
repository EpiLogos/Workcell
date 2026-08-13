export class WorkcellError extends Error {
  constructor(code, message, details = {}) {
    super(message);
    this.name = 'WorkcellError';
    this.code = code;
    this.details = details;
  }
}

export function invariant(condition, code, message, details = {}) {
  if (!condition) throw new WorkcellError(code, message, details);
}
