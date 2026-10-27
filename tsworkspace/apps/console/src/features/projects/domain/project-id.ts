import { create } from '@bufbuild/protobuf';
import { ProjectIdSchema, type ProjectId as ProjectIdMessage } from '@trogonai/proto';

import { invalidProject } from './faults';

declare const projectIdBrand: unique symbol;

export type ProjectId = string & { readonly [projectIdBrand]: true };

export const MAX_PROJECT_ID_LENGTH = 40;

const PROJECT_ID_PATTERN = /^[a-z][a-z0-9-]*$/;

export function projectIdProblem(value: string): string | undefined {
  if (value.length === 0) return 'A project id is required.';
  if (value.length > MAX_PROJECT_ID_LENGTH) return `A project id is at most ${MAX_PROJECT_ID_LENGTH} characters.`;
  if (!/^[a-z]/.test(value)) return 'A project id starts with a lowercase letter.';
  if (!PROJECT_ID_PATTERN.test(value)) return 'A project id uses only lowercase letters, digits and hyphens.';
  return undefined;
}

export function parseProjectId(value: string): ProjectId | undefined {
  return projectIdProblem(value) === undefined ? (value as ProjectId) : undefined;
}

export function mintProjectId(value: string): ProjectId {
  const problem = projectIdProblem(value);
  if (problem) throw invalidProject(problem);
  return value as ProjectId;
}

export function projectIdFromStored(value: string): ProjectId {
  return value as ProjectId;
}

export function projectIdMessage(id: ProjectId): ProjectIdMessage {
  return create(ProjectIdSchema, { value: id });
}

export function projectIdOf(message: ProjectIdMessage | undefined): ProjectId {
  return projectIdFromStored(message?.value ?? '');
}

export function suggestProjectId(displayName: string): string {
  return displayName
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^[^a-z]+/, '')
    .slice(0, MAX_PROJECT_ID_LENGTH)
    .replace(/-+$/, '');
}
