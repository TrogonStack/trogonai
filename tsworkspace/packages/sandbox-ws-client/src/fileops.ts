import { SandboxWsError } from './errors.ts';
import type { FileOpErrorKind, FileOpResponse } from './wire.ts';

/** Largest raw byte count one write request carries when uploading in chunks. */
export const FILE_TRANSFER_CHUNK_BYTES = 256 * 1024;

/** Most entries one directory listing carries. */
export const FILE_OP_MAX_ENTRIES = 20_000;

/** The provider behind the lease does not serve this file operation. */
export class FileOpUnsupportedError extends SandboxWsError {
  constructor(message: string) {
    super('unsupported', message);
    this.name = 'FileOpUnsupportedError';
  }
}

export class FileNotFoundError extends SandboxWsError {
  readonly path: string;

  constructor(path: string) {
    super('not_found', `no such file or directory: ${path}`);
    this.name = 'FileNotFoundError';
    this.path = path;
  }
}

export class FileOpFailedError extends SandboxWsError {
  readonly kind: FileOpErrorKind;

  constructor(kind: FileOpErrorKind, message: string) {
    super('file_op_failed', `${kind}: ${message}`, { variant: kind });
    this.name = 'FileOpFailedError';
    this.kind = kind;
  }
}

/** An atomic edit found a different occurrence count than expected; nothing was written. */
export class EditRejectedError extends SandboxWsError {
  readonly occurrences: number;
  readonly expected: number;

  constructor(occurrences: number, expected: number) {
    super('edit_rejected', `edit expected ${expected} occurrence(s) but found ${occurrences}; nothing was written`);
    this.name = 'EditRejectedError';
    this.occurrences = occurrences;
    this.expected = expected;
  }
}

const ERROR_KINDS: ReadonlySet<string> = new Set([
  'invalid_path',
  'permission_denied',
  'already_exists',
  'not_a_directory',
  'is_a_directory',
  'directory_not_empty',
  'too_large',
  'invalid_request',
  'io',
  'unknown_checkpoint',
  'unknown_watch',
]);

type Shape = Record<string, 'string' | 'number' | 'boolean' | 'array' | 'object'>;

const SHAPES: Record<string, Shape> = {
  listing: { entries: 'array', truncated: 'boolean' },
  stat: { stat: 'object' },
  content: { data: 'string', offset: 'number', total_size: 'number', eof: 'boolean', truncated: 'boolean' },
  written: { bytes: 'number', size: 'number' },
  edited: { occurrences: 'number', size: 'number' },
  edit_rejected: { occurrences: 'number', expected: 'number' },
  search_results: { matches: 'array', truncated: 'boolean', files_scanned: 'number' },
  removed: { existed: 'boolean' },
  created: { already_existed: 'boolean' },
  renamed: {},
  archive: { data: 'string', entries: 'number', bytes: 'number' },
  extracted: { entries: 'number', bytes: 'number' },
  checkpointed: { checkpoint_id: 'string', entries: 'number' },
  changes: { since: 'string', added: 'array', modified: 'array', deleted: 'array', truncated: 'boolean' },
  watch_started: { watch_id: 'string' },
  watch_events: { events: 'array', next_seq: 'number', overflowed: 'boolean' },
  watch_stopped: { existed: 'boolean' },
  checksum: { sha256: 'string', bytes: 'number', total_size: 'number' },
  not_found: { path: 'string' },
  failed: { kind: 'string', message: 'string' },
  unsupported: { message: 'string' },
};

function isKind(value: unknown, kind: Shape[string]): boolean {
  if (kind === 'array') return Array.isArray(value);
  if (kind === 'object') return typeof value === 'object' && value !== null && !Array.isArray(value);
  return typeof value === kind;
}

/** Checks a gateway reply against the `file_op` response shapes before the client trusts it. */
export function parseFileOpResponse(value: unknown): FileOpResponse {
  if (typeof value !== 'object' || value === null) {
    throw new SandboxWsError('invalid_response', 'file_op reply is not an object');
  }
  const record = value as Record<string, unknown>;
  const outcome = record.outcome;
  const shape = typeof outcome === 'string' ? SHAPES[outcome] : undefined;
  if (shape === undefined) {
    throw new SandboxWsError('invalid_response', `file_op reply has unknown outcome ${String(outcome)}`);
  }
  for (const [field, kind] of Object.entries(shape)) {
    if (!isKind(record[field], kind)) {
      throw new SandboxWsError('invalid_response', `file_op ${String(outcome)} reply has a bad ${field} field`);
    }
  }
  if (outcome === 'failed' && !ERROR_KINDS.has(record.kind as string)) {
    throw new SandboxWsError('invalid_response', `file_op failed reply has unknown kind ${String(record.kind)}`);
  }
  return value as FileOpResponse;
}

/** Turns a typed refusal into its error; every other outcome passes through. */
export function throwOnRefusal(response: FileOpResponse): FileOpResponse {
  switch (response.outcome) {
    case 'unsupported':
      throw new FileOpUnsupportedError(response.message);
    case 'not_found':
      throw new FileNotFoundError(response.path);
    case 'failed':
      throw new FileOpFailedError(response.kind, response.message);
    default:
      return response;
  }
}

export function bytesToBase64(bytes: Uint8Array): string {
  let binary = '';
  const step = 0x8000;
  for (let i = 0; i < bytes.length; i += step) {
    binary += String.fromCharCode(...bytes.subarray(i, i + step));
  }
  return btoa(binary);
}

export function base64ToBytes(text: string): Uint8Array {
  const binary = atob(text);
  const out = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i += 1) {
    out[i] = binary.charCodeAt(i);
  }
  return out;
}
