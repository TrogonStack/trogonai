/**
 * Safe shell quoting for the `$` tagged template (`sandbox.$`).
 *
 * Every interpolated value becomes exactly one shell word (an array becomes
 * one word per element), so a value cannot add arguments, end the command or
 * start a substitution. Quoting targets POSIX `sh`.
 */

const SAFE_WORD = /^[A-Za-z0-9_@%+:,./-]+$/;

/** Text inserted into a `$` template without quoting. The caller vouches for it. */
export class RawShell {
  constructor(readonly text: string) {}
  toString(): string {
    return this.text;
  }
}

/** Marks `text` as trusted shell syntax (a pipe, a redirect) so `$` leaves it unquoted. */
export function raw(text: string): RawShell {
  return new RawShell(text);
}

export type ShellValue = string | number | bigint | boolean | RawShell | readonly ShellValue[];

/**
 * Quotes one string as a single shell word. Words made only of letters,
 * digits and `_@%+:,./-` stay bare so logs stay readable; everything else,
 * including the empty string, is wrapped in single quotes with embedded
 * single quotes written as `'\''`. Newlines are literal inside single quotes.
 * A NUL byte cannot appear in an argument, so it throws.
 */
export function shellQuote(value: string): string {
  if (value.includes('\0')) {
    throw new TypeError('cannot quote a string containing a NUL byte');
  }
  if (value === '') {
    return "''";
  }
  if (SAFE_WORD.test(value)) {
    return value;
  }
  return `'${value.replaceAll("'", `'\\''`)}'`;
}

/**
 * Renders one interpolated value: strings are quoted, numbers and booleans
 * become their text, `raw()` text passes through, and an array becomes its
 * elements quoted and joined by spaces (an empty array renders as nothing).
 * `null`, `undefined`, objects, functions and symbols throw `TypeError`,
 * because guessing a rendering for them is how injection bugs start.
 */
export function shellValue(value: unknown): string {
  if (value instanceof RawShell) {
    return value.text;
  }
  if (typeof value === 'string') {
    return shellQuote(value);
  }
  if (typeof value === 'number') {
    if (!Number.isFinite(value)) {
      throw new TypeError(`cannot interpolate ${String(value)} into a shell command`);
    }
    return shellQuote(String(value));
  }
  if (typeof value === 'bigint' || typeof value === 'boolean') {
    return shellQuote(String(value));
  }
  if (Array.isArray(value)) {
    return value.map((item) => shellValue(item)).join(' ');
  }
  throw new TypeError(`cannot interpolate a ${value === null ? 'null' : typeof value} into a shell command`);
}

/** Builds a shell command string from a template literal, quoting every interpolation. */
export function shellTemplate(strings: TemplateStringsArray, ...values: unknown[]): string {
  let out = strings[0] ?? '';
  for (let i = 0; i < values.length; i += 1) {
    out += shellValue(values[i]) + (strings[i + 1] ?? '');
  }
  return out;
}
