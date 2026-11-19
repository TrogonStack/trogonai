import { z } from 'zod';

/** What a registered value is replaced with. Matches `sandbox_core::redact::REDACTION_MARKER`. */
export const REDACTION_MARKER = '[REDACTED]';
/** Values shorter than this (in UTF-8 bytes) are ignored by the worker. */
export const MIN_REDACTABLE_LEN = 6;
export const MAX_REDACT_VALUES = 64;
export const MAX_REDACT_VALUE_LEN = 8 * 1024;

const utf8Length = (s: string): number => new TextEncoder().encode(s).length;

/**
 * Literal values to scrub from command output. Exact bytes only: an encoded
 * or transformed copy of a value (base64, hex, split across lines) is not
 * caught.
 */
export const redactValuesSchema = z
  .array(z.string().refine((s) => utf8Length(s) <= MAX_REDACT_VALUE_LEN, `value exceeds ${MAX_REDACT_VALUE_LEN} bytes`))
  .max(MAX_REDACT_VALUES);

/**
 * Leftmost-longest, non-overlapping replacement of `values` in `text`, the
 * same rule the workers apply. Values shorter than `MIN_REDACTABLE_LEN` are ignored.
 */
export function redactText(text: string, values: readonly string[]): string {
  const usable = [...new Set(values.filter((v) => utf8Length(v) >= MIN_REDACTABLE_LEN))].sort(
    (a, b) => b.length - a.length,
  );
  if (usable.length === 0) {
    return text;
  }
  let out = '';
  let i = 0;
  while (i < text.length) {
    const hit = usable.find((v) => text.startsWith(v, i));
    if (hit === undefined) {
      out += text[i];
      i += 1;
    } else {
      out += REDACTION_MARKER;
      i += hit.length;
    }
  }
  return out;
}
