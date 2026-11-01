import { SourceFault } from './faults';
import { isSourceKind, type SourceKind } from './source-kind';

declare const integrationIdBrand: unique symbol;
declare const subjectPrefixBrand: unique symbol;

export type IntegrationId = string & { readonly [integrationIdBrand]: true };
export type NatsSubjectPrefix = string & { readonly [subjectPrefixBrand]: true };

const INSTANCE_PATTERN = /^[a-z][a-z0-9-]*$/;

export function integrationIdProblem(value: string): string | undefined {
  const [kind, instance, ...rest] = value.split('/');
  if (kind === undefined || instance === undefined || rest.length > 0) {
    return 'An integration id is a source kind and an instance name separated by a slash.';
  }
  if (!isSourceKind(kind)) return `${kind} is not a known source kind.`;
  if (!INSTANCE_PATTERN.test(instance)) {
    return 'An instance name starts with a lowercase letter and uses only lowercase letters, digits and hyphens.';
  }
  return undefined;
}

export function parseIntegrationId(value: string): IntegrationId | undefined {
  return integrationIdProblem(value) === undefined ? (value as IntegrationId) : undefined;
}

export function mintIntegrationId(value: string): IntegrationId {
  const problem = integrationIdProblem(value);
  if (problem) throw new SourceFault('COMMAND_INVALID', problem);
  return value as IntegrationId;
}

export function kindOf(id: IntegrationId): SourceKind {
  return id.split('/')[0] as SourceKind;
}

export function instanceOf(id: IntegrationId): string {
  return id.split('/')[1]!;
}

export function subjectPrefixOf(id: IntegrationId): NatsSubjectPrefix {
  return `trogon.${kindOf(id)}.${instanceOf(id)}` as NatsSubjectPrefix;
}
