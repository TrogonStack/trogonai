/** Replaces every secret value that `scrubSecrets` removes. */
export const REDACTED = '[REDACTED]';

// Headers whose name alone says the value is a credential.
const CREDENTIAL_HEADERS = 'x-sandbox-api-key|x-api-key|api-key|x-auth-token|x-amz-security-token|cookie|set-cookie';

const SENSITIVE_KEYS =
  'access[_-]?token|refresh[_-]?token|id[_-]?token|auth[_-]?token|token|api[_-]?key|apikey|key|secret|client[_-]?secret|password|passwd|pwd|signature|sig|x-amz-signature|x-amz-credential|x-amz-security-token';

// Order matters: header lines first so a whole `Authorization: Basic ...`
// value goes in one piece, then the narrower token shapes.
const RULES: ReadonlyArray<readonly [RegExp, string]> = [
  // `X-Sandbox-API-Key: ...` up to the end of the line or a closing quote.
  [new RegExp(`\\b(${CREDENTIAL_HEADERS})(\\s*[:=]\\s*)[^\\r\\n"']+`, 'gi'), `$1$2${REDACTED}`],
  // `Authorization: <scheme> <credentials>`, or a bare token of 16 or more characters.
  // Plain prose such as `authorization: denied` is left alone.
  [
    /\b((?:proxy-)?authorization\s*[:=]\s*)(?:(?:bearer|basic|token|digest|negotiate|aws4-hmac-sha256)\s+[^\r\n"']+|[A-Za-z0-9\-._~+/=]{16,})/gi,
    `$1${REDACTED}`,
  ],
  // `scheme://user:pass@host` and `scheme://token@host`.
  [/\b([a-z][a-z0-9+.-]*:\/\/)[^\s/?#@]+@/gi, `$1${REDACTED}@`],
  // `Bearer abc`, and the WebSocket subprotocol form `bearer.abc`.
  [/\b(bearer)([\s.]+)(?!\[REDACTED\])[A-Za-z0-9\-._~+/=]{8,}/gi, `$1$2${REDACTED}`],
  // `?token=abc&other=1`, also `;key=abc` and form bodies.
  [new RegExp(`([?&;])(${SENSITIVE_KEYS})=[^&\\s"'#]*`, 'gi'), `$1$2=${REDACTED}`],
  // `"token": "abc"` inside a JSON error body.
  [new RegExp(`("(?:${SENSITIVE_KEYS})"\\s*:\\s*)"(?:[^"\\\\]|\\\\.)*"`, 'gi'), `$1"${REDACTED}"`],
];

/**
 * Removes credentials from free text: URL userinfo, bearer tokens, sensitive
 * query parameters, API-key and authorization headers, and sensitive JSON
 * string fields. Pure and deterministic, and idempotent: scrubbing already
 * scrubbed text returns it unchanged. It is a best-effort filter over known
 * shapes, not a guarantee that no secret survives in arbitrary text.
 */
export function scrubSecrets(text: string): string {
  let out = text;
  for (const [pattern, replacement] of RULES) {
    out = out.replace(pattern, replacement);
  }
  return out;
}

/**
 * A copy of `cause` that is safe to attach to an error: an `Error` keeps its
 * name and gets a scrubbed message and stack; anything else becomes scrubbed
 * text. The original object is never kept, since its own fields could hold a
 * secret.
 */
export function scrubCause(cause: unknown): unknown {
  if (cause === undefined || cause === null) {
    return cause;
  }
  if (cause instanceof Error) {
    const copy = new Error(
      scrubSecrets(cause.message),
      cause.cause === undefined ? undefined : { cause: scrubCause(cause.cause) },
    );
    copy.name = cause.name;
    copy.stack = cause.stack === undefined ? undefined : scrubSecrets(cause.stack);
    return copy;
  }
  return scrubSecrets(typeof cause === 'string' ? cause : String(cause));
}
