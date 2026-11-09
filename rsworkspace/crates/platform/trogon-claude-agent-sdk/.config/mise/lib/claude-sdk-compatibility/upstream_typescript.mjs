import { createHash } from 'node:crypto';
import ts from 'typescript';

export class GateError extends Error {
  constructor(code, selector) {
    super(`${code}: ${selector}`);
    this.name = 'GateError';
    this.code = code;
    this.selector = selector;
  }
}

const unsupported = selector => { throw new GateError('unsupported-format', selector); };

export function extract(contents, expectedHash) {
  unsupported("typescript-declarations");
}
