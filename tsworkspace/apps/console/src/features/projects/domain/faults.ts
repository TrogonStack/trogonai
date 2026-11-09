import { getOption } from '@bufbuild/protobuf';
import type { DescMessage } from '@bufbuild/protobuf';
import {
  ProjectAlreadyExistsErrorSchema,
  ProjectArchivedErrorSchema,
  ProjectCommandInvalidErrorSchema,
  ProjectNotArchivedErrorSchema,
  ProjectNotFoundErrorSchema,
  errorTemplateOption,
} from '@trogonai/proto';

const faultSchemas = {
  PROJECT_COMMAND_INVALID: ProjectCommandInvalidErrorSchema,
  PROJECT_ALREADY_EXISTS: ProjectAlreadyExistsErrorSchema,
  PROJECT_NOT_FOUND: ProjectNotFoundErrorSchema,
  PROJECT_ARCHIVED: ProjectArchivedErrorSchema,
  PROJECT_NOT_ARCHIVED: ProjectNotArchivedErrorSchema,
} as const satisfies Record<string, DescMessage>;

export type ProjectDomainReason = keyof typeof faultSchemas;

const concurrencyMessages = {
  REVISION_MISMATCH: 'The project changed since it was loaded. Reload and try again.',
  EXPECTED_REVISION_REQUIRED: 'Commands on an existing project require a positive expected revision.',
} as const;

export type ProjectConcurrencyReason = keyof typeof concurrencyMessages;

export type ProjectFaultReason = ProjectDomainReason | ProjectConcurrencyReason;

export function projectFaultMessage(reason: ProjectFaultReason): string {
  if (reason in concurrencyMessages) return concurrencyMessages[reason as ProjectConcurrencyReason];
  return getOption(faultSchemas[reason as ProjectDomainReason], errorTemplateOption).template?.message ?? reason;
}

export class ProjectFault extends Error {
  readonly kind = 'domain' as const;

  constructor(
    readonly reason: ProjectFaultReason,
    readonly detail?: string,
  ) {
    super(detail ? `${projectFaultMessage(reason)} ${detail}` : projectFaultMessage(reason));
    this.name = 'ProjectFault';
  }
}

export function isProjectFault(error: unknown): error is ProjectFault {
  return error instanceof ProjectFault;
}

export function invalidProject(detail: string): ProjectFault {
  return new ProjectFault('PROJECT_COMMAND_INVALID', detail);
}
