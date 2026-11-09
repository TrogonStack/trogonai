import { EnvironmentFault } from './faults';

declare const environmentIdBrand: unique symbol;

export type EnvironmentId = string & { readonly [environmentIdBrand]: true };

export const MAX_ENVIRONMENT_ID_LENGTH = 32;

const ENVIRONMENT_ID_PATTERN = /^[a-z][a-z0-9-]*$/;

export const builtInEnvironmentIds = {
  production: mintEnvironmentId('production'),
  preview: mintEnvironmentId('preview'),
  development: mintEnvironmentId('development'),
} as const;

const builtIns: ReadonlySet<string> = new Set(Object.values(builtInEnvironmentIds));

export function environmentIdProblem(value: string): string | undefined {
  if (value.length === 0) return 'An environment id is required.';
  if (value.length > MAX_ENVIRONMENT_ID_LENGTH) {
    return `An environment id is at most ${MAX_ENVIRONMENT_ID_LENGTH} characters.`;
  }
  if (!/^[a-z]/.test(value)) return 'An environment id starts with a lowercase letter.';
  if (!ENVIRONMENT_ID_PATTERN.test(value)) {
    return 'An environment id uses only lowercase letters, digits and hyphens.';
  }
  return undefined;
}

export function parseEnvironmentId(value: string): EnvironmentId | undefined {
  return environmentIdProblem(value) === undefined ? (value as EnvironmentId) : undefined;
}

export function isBuiltInEnvironmentId(id: EnvironmentId): boolean {
  return builtIns.has(id);
}

export function environmentIdFromStored(value: string): EnvironmentId {
  return value as EnvironmentId;
}

export function mintEnvironmentId(value: string): EnvironmentId {
  const problem = environmentIdProblem(value);
  if (problem) throw new EnvironmentFault('COMMAND_INVALID', problem);
  return value as EnvironmentId;
}

export function suggestEnvironmentId(displayName: string): string {
  return displayName
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^[^a-z]+/, '')
    .slice(0, MAX_ENVIRONMENT_ID_LENGTH)
    .replace(/-+$/, '');
}
