import { FileOpFailedError } from './fileops.ts';

/** One regular file of a directory transfer. `path` is relative to the transferred directory. */
export interface DirFile {
  path: string;
  data: Uint8Array;
  /** Unix permission bits. */
  mode?: number;
}

/** What one file or tree transfer moved. */
export interface TransferReport {
  /** Regular files that now match the source. */
  files: number;
  /** Directories created or confirmed. */
  directories: number;
  /** Bytes sent over the wire; resumed bytes are not counted. */
  bytesTransferred: number;
  /** Files continued from a partial copy instead of restarted. */
  resumedFiles: number;
  /** Files already complete and identical, so nothing moved. */
  skippedFiles: number;
}

/** The result of `downloadDir`. */
export interface DownloadedDir {
  files: DirFile[];
  /** Directory paths relative to the downloaded directory, empty ones included. */
  directories: string[];
  report: TransferReport;
}

/** SHA-256 of a byte string as lowercase hex, through WebCrypto. */
export async function sha256Hex(bytes: Uint8Array): Promise<string> {
  const digest = await globalThis.crypto.subtle.digest('SHA-256', bytes as Uint8Array<ArrayBuffer>);
  let hex = '';
  for (const byte of new Uint8Array(digest)) {
    hex += byte.toString(16).padStart(2, '0');
  }
  return hex;
}

export function emptyReport(): TransferReport {
  return { files: 0, directories: 0, bytesTransferred: 0, resumedFiles: 0, skippedFiles: 0 };
}

export function absorb(
  report: TransferReport,
  file: { bytesTransferred: number; resumed: boolean; skipped: boolean },
): void {
  report.files += 1;
  report.bytesTransferred += file.bytesTransferred;
  if (file.resumed) report.resumedFiles += 1;
  if (file.skipped) report.skippedFiles += 1;
}

/** Joins a listing-relative path onto a sandbox directory. */
export function joinRemote(base: string, relative: string): string {
  const tail = relative.replace(/^\/+/, '');
  return base.endsWith('/') ? `${base}${tail}` : `${base}/${tail}`;
}

/** Refuses a transfer path that is absolute or climbs out of the directory. */
export function checkRelative(path: string): string {
  const parts = path.split('/').filter((part) => part !== '' && part !== '.');
  if (path.startsWith('/') || parts.length === 0 || parts.includes('..')) {
    throw new FileOpFailedError('invalid_path', `${path} is not a path inside the transferred directory`);
  }
  return parts.join('/');
}

export function integrityError(path: string): FileOpFailedError {
  return new FileOpFailedError('io', `integrity check failed for ${path}: contents differ from the source`);
}

export function resumeMismatchError(path: string): FileOpFailedError {
  return new FileOpFailedError(
    'io',
    `cannot resume ${path}: the sandbox copy is not a prefix of the source; retry without resume`,
  );
}
