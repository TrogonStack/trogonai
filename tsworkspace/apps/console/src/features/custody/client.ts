import type { PurposeCustody } from '@trogonai/proto';

import type { AttemptId, KeyBackendId, KeyRef, MigrationId, PurposeId } from './domain/identifiers';
import type { TenantKey } from './domain/key';
import type { CustodyTarget, Migration } from './domain/migration';
import type { BindingInput, RegistrationInput } from './domain/registration';
import type { BackendRecord, TenantCustodyState } from './domain/state';
import type { BindingPlan, CheckResult, RegistrationPlan } from './domain/validation';

export interface StartMigrationInput {
  readonly purpose: PurposeId;
  readonly target: CustodyTarget;
}

export interface StartMigrationResult {
  readonly custody: PurposeCustody;
  readonly migration: Migration | undefined;
}

export interface TenantCustodyClient {
  load(): Promise<TenantCustodyState>;
  prepareRegistration(input: RegistrationInput): Promise<RegistrationPlan>;
  prepareBinding(input: BindingInput): Promise<BindingPlan>;
  runCheck(attemptId: AttemptId, checkId: string): Promise<CheckResult>;
  completeRegistration(attemptId: AttemptId): Promise<BackendRecord>;
  completeBinding(attemptId: AttemptId): Promise<TenantKey>;
  validateBackend(backendId: KeyBackendId): Promise<BackendRecord>;
  pauseBackend(backendId: KeyBackendId): Promise<BackendRecord>;
  resumeBackend(backendId: KeyBackendId): Promise<BackendRecord>;
  setDefaultKey(keyRef: KeyRef): Promise<void>;
  startMigration(input: StartMigrationInput): Promise<StartMigrationResult>;
  pauseMigration(migrationId: MigrationId): Promise<Migration>;
  resumeMigration(migrationId: MigrationId): Promise<Migration>;
}
