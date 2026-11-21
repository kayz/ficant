import type { PrincipalRole } from '../generated/ficant/blotter/v1/model_pb';

export const AUTHORIZATION_HEADER = 'authorization';
export const FICANT_CREDENTIAL_HEADER = 'x-ficant-credential';

export type CredentialHeaderMode = 'authorization' | 'x-ficant-credential';

export interface LocationLike {
  readonly protocol: string;
  readonly hostname: string;
  readonly origin?: string | undefined;
  readonly port?: string | undefined;
}

export interface InMemoryCredentialInput {
  readonly token: string;
  readonly headerMode: CredentialHeaderMode;
  readonly roleHint?: PrincipalRole | undefined;
  readonly actorLabel?: string | undefined;
}

export interface CredentialMetadata {
  readonly headerMode: CredentialHeaderMode;
  readonly roleHint: PrincipalRole | null;
  readonly actorLabel: string | null;
}

export class InsecureHttpContextError extends Error {
  readonly code = 'INSECURE_HTTP_CONTEXT' as const;

  constructor(originDescription: string) {
    super(
      `INSECURE_HTTP_CONTEXT: credential entry and transmission are blocked over non-loopback plain HTTP (${originDescription}). Use HTTPS.`,
    );
    this.name = 'InsecureHttpContextError';
  }
}

export class CredentialValidationError extends Error {
  readonly code = 'INVALID_CREDENTIAL' as const;

  constructor(message: string) {
    super(message);
    this.name = 'CredentialValidationError';
  }
}

const IPV4_LOOPBACK_PATTERN = /^127(?:\.(?:25[0-5]|2[0-4]\d|1?\d?\d)){3}$/;

export function isLoopbackHostname(hostname: string): boolean {
  const normalized = hostname.trim().toLowerCase();
  if (
    normalized === 'localhost' ||
    normalized === '::1' ||
    normalized === '[::1]' ||
    normalized === '127.0.0.1'
  ) {
    return true;
  }
  return IPV4_LOOPBACK_PATTERN.test(normalized);
}

/**
 * Enforces that credentials are only accepted/sent over HTTPS or loopback HTTP.
 * Non-loopback plain `http://` is blocked BEFORE credential entry with `INSECURE_HTTP_CONTEXT`.
 */
export function assertSecureCredentialContext(locationLike?: LocationLike): void {
  const loc =
    locationLike ??
    (typeof globalThis.location === 'object' && globalThis.location !== null
      ? {
          protocol: globalThis.location.protocol,
          hostname: globalThis.location.hostname,
        }
      : undefined);

  if (!loc) {
    return;
  }

  const protocol = loc.protocol.toLowerCase();
  if (protocol === 'https:') {
    return;
  }
  if (protocol === 'http:' && isLoopbackHostname(loc.hostname)) {
    return;
  }
  throw new InsecureHttpContextError(`${loc.protocol}//${loc.hostname}`);
}

export function isSecureCredentialContext(locationLike?: LocationLike): boolean {
  try {
    assertSecureCredentialContext(locationLike);
    return true;
  } catch {
    return false;
  }
}

/**
 * Validates raw credential token matching `crates/ficant-api/src/auth.rs::validate_raw_token`:
 * - Non-empty
 * - Zero ASCII whitespace, zero ASCII control chars, visible ASCII only (0x21..=0x7e)
 */
export function validateRawCredentialToken(token: string): string {
  if (token.length === 0) {
    throw new CredentialValidationError('Credential token must not be empty.');
  }
  for (let i = 0; i < token.length; i += 1) {
    const code = token.charCodeAt(i);
    if (code <= 0x20 || code >= 0x7f) {
      throw new CredentialValidationError(
        'Credential token must not contain whitespace, control characters, or non-ASCII bytes.',
      );
    }
  }
  return token;
}

/**
 * Builds the single mutually-exclusive credential header object for a request.
 */
export function buildCredentialHeader(
  token: string,
  headerMode: CredentialHeaderMode,
): Readonly<Record<string, string>> {
  const validToken = validateRawCredentialToken(token);
  if (headerMode === 'authorization') {
    return { [AUTHORIZATION_HEADER]: `Bearer ${validToken}` };
  }
  if (headerMode === 'x-ficant-credential') {
    return { [FICANT_CREDENTIAL_HEADER]: validToken };
  }
  throw new CredentialValidationError(`Unsupported credential header mode: ${String(headerMode)}`);
}

/**
 * Validates outgoing headers to enforce strict mutual exclusion matching `auth.rs`:
 * - Exactly one of `authorization` or `x-ficant-credential`
 * - Rejects missing, duplicate, or conflicting headers
 * - `authorization` must start with exact `"Bearer "` followed by a valid raw token
 * - `x-ficant-credential` accepts raw token or `"Bearer <token>"` with valid raw token
 */
export function assertExclusiveCredentialHeaders(
  headers: Headers | Readonly<Record<string, string | readonly string[] | undefined>>,
): { readonly mode: CredentialHeaderMode; readonly token: string } {
  const authValues: string[] = [];
  const customValues: string[] = [];

  if (headers instanceof Headers) {
    // Note: Fetch Headers coalesces multiple values with ", ".
    const auth = headers.get(AUTHORIZATION_HEADER);
    if (auth !== null) {
      authValues.push(auth);
    }
    const custom = headers.get(FICANT_CREDENTIAL_HEADER);
    if (custom !== null) {
      customValues.push(custom);
    }
  } else {
    for (const [rawKey, rawVal] of Object.entries(headers)) {
      if (rawVal === undefined) {
        continue;
      }
      const key = rawKey.toLowerCase();
      const list = Array.isArray(rawVal) ? rawVal : [rawVal];
      if (key === AUTHORIZATION_HEADER) {
        authValues.push(...list);
      } else if (key === FICANT_CREDENTIAL_HEADER) {
        customValues.push(...list);
      }
    }
  }

  if (authValues.length > 0 && customValues.length > 0) {
    throw new CredentialValidationError(
      'Conflicting credential headers: authorization and x-ficant-credential are mutually exclusive.',
    );
  }
  if (authValues.length > 1 || customValues.length > 1) {
    throw new CredentialValidationError('Duplicate credential headers are not permitted.');
  }

  if (authValues.length === 1) {
    const raw = authValues[0] ?? '';
    if (!raw.startsWith('Bearer ')) {
      throw new CredentialValidationError(
        'authorization header must start with exact "Bearer " prefix.',
      );
    }
    const token = validateRawCredentialToken(raw.slice('Bearer '.length));
    return { mode: 'authorization', token };
  }

  if (customValues.length === 1) {
    const raw = customValues[0] ?? '';
    const stripped = raw.startsWith('Bearer ') ? raw.slice('Bearer '.length) : raw;
    const token = validateRawCredentialToken(stripped);
    return { mode: 'x-ficant-credential', token };
  }

  throw new CredentialValidationError('Missing credential header.');
}

interface StoredCredentialSecret {
  readonly token: string;
  readonly headerMode: CredentialHeaderMode;
  readonly roleHint: PrincipalRole | null;
  readonly actorLabel: string | null;
}

/**
 * Strictly in-memory credential holder.
 * Never persists tokens to localStorage, sessionStorage, cookies, IndexedDB, or URL.
 */
export class InMemoryCredentialStore {
  #secret: StoredCredentialSecret | null = null;

  setCredential(input: InMemoryCredentialInput, locationLike?: LocationLike): CredentialMetadata {
    assertSecureCredentialContext(locationLike);
    const validToken = validateRawCredentialToken(input.token);
    if (input.headerMode !== 'authorization' && input.headerMode !== 'x-ficant-credential') {
      throw new CredentialValidationError(
        `Unsupported credential header mode: ${String(input.headerMode)}`,
      );
    }
    this.#secret = {
      token: validToken,
      headerMode: input.headerMode,
      roleHint: input.roleHint ?? null,
      actorLabel:
        input.actorLabel !== undefined && input.actorLabel.trim().length > 0
          ? input.actorLabel.trim()
          : null,
    };
    return {
      headerMode: this.#secret.headerMode,
      roleHint: this.#secret.roleHint,
      actorLabel: this.#secret.actorLabel,
    };
  }

  clear(): void {
    this.#secret = null;
  }

  hasCredential(): boolean {
    return this.#secret !== null;
  }

  getMetadata(): CredentialMetadata | null {
    if (!this.#secret) {
      return null;
    }
    return {
      headerMode: this.#secret.headerMode,
      roleHint: this.#secret.roleHint,
      actorLabel: this.#secret.actorLabel,
    };
  }

  applyToHeaders(target: Headers): void {
    target.delete(AUTHORIZATION_HEADER);
    target.delete(FICANT_CREDENTIAL_HEADER);
    if (!this.#secret) {
      throw new CredentialValidationError('No active in-memory credential.');
    }
    const headerEntry = buildCredentialHeader(this.#secret.token, this.#secret.headerMode);
    for (const [k, v] of Object.entries(headerEntry)) {
      target.set(k, v);
    }
    assertExclusiveCredentialHeaders(target);
  }

  getRedactedSecrets(): readonly string[] {
    return this.#secret ? [this.#secret.token] : [];
  }

  toJSON(): { readonly authenticated: boolean; readonly headerMode: CredentialHeaderMode | null } {
    return {
      authenticated: this.#secret !== null,
      headerMode: this.#secret?.headerMode ?? null,
    };
  }

  toString(): string {
    return `InMemoryCredentialStore(authenticated=${String(this.#secret !== null)})`;
  }
}
