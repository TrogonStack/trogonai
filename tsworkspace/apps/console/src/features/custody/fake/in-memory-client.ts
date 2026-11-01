import { create } from '@bufbuild/protobuf';
import { timestampFromDate } from '@bufbuild/protobuf/wkt';
import {
  KeyBackendAdmission,
  KeyBackendFailureReason,
  KeyBackendLifecycle,
  KeyCapability,
  KeyProvider,
  KeyState,
  KeyStateObservationSchema,
  KeyUseDenialReason,
} from '@trogonai/proto';

import type { StartMigrationInput, StartMigrationResult, TenantCustodyClient } from '../client';
import { keyUseDenial, registrationDenial } from '../domain/admission';
import { failureReasonLabels, providerOfAccess } from '../domain/backend';
import {
  customerManagedCustody,
  dedicatedCustody,
  keyRefOfCustody,
  managedCustody,
  purposeOfCustody,
} from '../domain/custody';
import { KeyFault, keyUseDenied } from '../domain/faults';
import {
  attemptId,
  externalId,
  hexOf,
  keyBackendId,
  keyBackendIdOf,
  keyRef,
  migrationId,
  parseKeyBackendId,
  type AttemptId,
  type ExternalId,
  type KeyBackendId,
  type KeyRef,
  type MigrationId,
  type PurposeId,
} from '../domain/identifiers';
import { emptyUsage, type TenantKey } from '../domain/key';
import { buildLocator, locatorProblem, locatorText } from '../domain/locator';
import {
  advanceMigration,
  endpointKeyRef,
  isInFlight,
  sameEndpoint,
  type CustodyEndpoint,
  type CustodyTarget,
  type Migration,
} from '../domain/migration';
import {
  bindingProblems,
  boundaryKeyOfAccess,
  buildAccess,
  registrationProblems,
  type BindingInput,
  type RegistrationInput,
} from '../domain/registration';
import type { BackendRecord, TenantCustodyState } from '../domain/state';
import {
  bindingChecks,
  registrationChecks,
  type BindingPlan,
  type CheckDefinition,
  type CheckResult,
  type RegistrationPlan,
} from '../domain/validation';
import { evaluateBindingCheck, evaluateRegistrationCheck } from './evaluate';
import { isPlatformOwned, projectForTenant, type InternalCustodyState } from './internal-state';
import { healthOf, registrationOf, stateObservation, withRegistration, bindingDetail } from './proto-builders';
import { fakePurposeCatalogue } from './purpose-catalogue';
import { seedBackendIds } from './seed-ids';
import { instructionsFor } from './setup-instructions';

const MIGRATION_TARGET_MINUTES = 3;

const emptyState: InternalCustodyState = {
  backends: [],
  purposes: fakePurposeCatalogue,
  custody: [],
  keys: [],
  payloads: [],
  migrations: [],
  operations: [],
  placements: new Map(),
};

export interface InMemoryTenantCustodyOptions {
  readonly latencyMs?: number;
  readonly clock?: () => Date;
}

interface Attempt {
  readonly checks: readonly CheckDefinition[];
  readonly results: Map<string, CheckResult>;
  readonly runs: Map<string, number>;
}

interface RegistrationAttempt extends Attempt {
  readonly kind: 'registration';
  readonly input: RegistrationInput;
  readonly externalId: ExternalId | undefined;
}

interface BindingAttempt extends Attempt {
  readonly kind: 'binding';
  readonly backendId: KeyBackendId;
  readonly input: BindingInput;
}

function sleep(milliseconds: number): Promise<void> {
  return milliseconds > 0 ? new Promise((resolve) => setTimeout(resolve, milliseconds)) : Promise.resolve();
}

function hash(value: string): number {
  let result = 2166136261;
  for (const character of value) result = Math.imul(result ^ character.charCodeAt(0), 16777619);
  return result >>> 0;
}

function slug(value: string): string {
  return value
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '_')
    .replace(/^_+|_+$/g, '')
    .slice(0, 24);
}

export class InMemoryTenantCustodyClient implements TenantCustodyClient {
  private state: InternalCustodyState = emptyState;
  private readonly attempts = new Map<AttemptId, RegistrationAttempt | BindingAttempt>();
  private readonly outages = new Map<KeyBackendId, KeyBackendFailureReason>();
  private readonly latencyMs: number;
  private readonly clock: () => Date;
  private seed: () => InternalCustodyState = () => emptyState;
  private ready: Promise<void> | undefined;
  private counter = 0;

  constructor(options: InMemoryTenantCustodyOptions = {}) {
    this.latencyMs = options.latencyMs ?? 0;
    this.clock = options.clock ?? (() => new Date());
  }

  seedWith(populate: () => InternalCustodyState): void {
    this.seed = populate;
  }

  async simulateProviderOutage(backendId: KeyBackendId, reason: KeyBackendFailureReason | undefined): Promise<void> {
    await this.settled();
    if (reason === undefined) this.outages.delete(backendId);
    else this.outages.set(backendId, reason);
  }

  async simulateKeyState(ref: KeyRef, state: KeyState): Promise<void> {
    await this.settled();
    const now = this.clock();
    this.state = {
      ...this.state,
      keys: this.state.keys.map((key) => {
        if (key.keyRef !== ref) return key;
        const observation = stateObservation(state, now);
        return {
          ...key,
          state: observation,
          binding:
            key.binding.kind === 'customer_managed'
              ? { kind: 'customer_managed', detail: { ...key.binding.detail, state: observation } }
              : key.binding,
        };
      }),
    };
  }

  async platformBackendCount(): Promise<number> {
    await this.settled();
    return this.state.backends.filter(isPlatformOwned).length;
  }

  private async settled(): Promise<void> {
    this.ready ??= Promise.resolve().then(() => {
      this.state = this.seed();
    });
    await this.ready;
  }

  private async pause(): Promise<void> {
    await this.settled();
    await sleep(this.latencyMs);
    this.settle();
  }

  private settle(): void {
    const now = this.clock();
    let payloads = this.state.payloads;
    const migrations = this.state.migrations.map((migration) => {
      if (migration.status !== 'running') return migration;
      if (this.migrationDenial(migration) !== undefined) return { ...migration, updatedAt: now };
      const advanced = advanceMigration(migration, now);
      if (advanced.status === 'completed') {
        const from = endpointKeyRef(migration.from);
        const to = endpointKeyRef(migration.to);
        payloads = payloads.map((set) =>
          set.purpose === migration.purpose && set.keyRef === from ? { ...set, keyRef: to } : set,
        );
      }
      return advanced;
    });
    this.state = { ...this.state, migrations, payloads };
  }

  private next(prefix: string): number {
    this.counter += 1;
    return hash(`${prefix}:${this.counter}`);
  }

  private tenantBackend(id: string): BackendRecord {
    const parsed = parseKeyBackendId(id);
    const record = this.state.backends.find(
      (candidate) => !isPlatformOwned(candidate) && keyBackendIdOf(candidate.registration.keyBackendId) === parsed,
    );
    if (!record) throw new KeyFault('BACKEND_NOT_FOUND');
    return record;
  }

  private replaceBackend(record: BackendRecord): BackendRecord {
    const id = keyBackendIdOf(record.registration.keyBackendId);
    this.state = {
      ...this.state,
      backends: this.state.backends.map((candidate) =>
        keyBackendIdOf(candidate.registration.keyBackendId) === id ? record : candidate,
      ),
    };
    return record;
  }

  private keyOf(ref: KeyRef): TenantKey {
    const key = this.state.keys.find((candidate) => candidate.keyRef === ref);
    if (!key) throw new KeyFault('KEY_NOT_FOUND');
    return key;
  }

  private attemptOf(id: AttemptId): RegistrationAttempt | BindingAttempt {
    const attempt = this.attempts.get(id);
    if (!attempt) throw new KeyFault('ATTEMPT_NOT_FOUND');
    return attempt;
  }

  private admissionOf(key: TenantKey): KeyBackendAdmission {
    if (key.binding.kind !== 'customer_managed') return KeyBackendAdmission.ENABLED;
    const owner = keyBackendIdOf(key.binding.detail.keyBackendId);
    const record = this.state.backends.find(
      (candidate) => keyBackendIdOf(candidate.registration.keyBackendId) === owner,
    );
    return record?.registration.admission ?? KeyBackendAdmission.ENABLED;
  }

  private denialOf(ref: KeyRef, operation: 'wrap' | 'unwrap'): KeyUseDenialReason | undefined {
    const key = this.keyOf(ref);
    return keyUseDenial(this.admissionOf(key), key.state.state, operation);
  }

  private requireUse(ref: KeyRef, operation: 'wrap' | 'unwrap'): void {
    const denial = this.denialOf(ref, operation);
    if (denial !== undefined) throw keyUseDenied(denial);
  }

  private migrationDenial(migration: Migration): KeyUseDenialReason | undefined {
    const from = endpointKeyRef(migration.from);
    const to = endpointKeyRef(migration.to);
    return (from ? this.denialOf(from, 'unwrap') : undefined) ?? (to ? this.denialOf(to, 'wrap') : undefined);
  }

  private endpointOf(purpose: PurposeId): CustodyEndpoint {
    const custody = this.state.custody.find((candidate) => purposeOfCustody(candidate) === purpose);
    const ref = custody ? keyRefOfCustody(custody) : undefined;
    if (!custody || !ref) return { tier: 'managed' };
    return { tier: custody.custody.case === 'dedicated' ? 'dedicated' : 'customer_managed', keyRef: ref };
  }

  private setCustody(purpose: PurposeId, endpoint: CustodyEndpoint): StartMigrationResult['custody'] {
    const ref = endpointKeyRef(endpoint);
    let custody = managedCustody(purpose);
    if (ref && endpoint.tier === 'dedicated') custody = dedicatedCustody(purpose, ref);
    if (ref && endpoint.tier === 'customer_managed') {
      const key = this.keyOf(ref);
      if (key.binding.kind === 'customer_managed') {
        const owner = keyBackendIdOf(key.binding.detail.keyBackendId);
        const backend = this.state.backends.find(
          (record) => keyBackendIdOf(record.registration.keyBackendId) === owner,
        );
        custody = customerManagedCustody(
          purpose,
          ref,
          owner,
          providerOfAccess(backend?.registration.configuration?.access),
        );
      }
    }
    this.state = {
      ...this.state,
      custody: [...this.state.custody.filter((candidate) => purposeOfCustody(candidate) !== purpose), custody],
    };
    return custody;
  }

  async load(): Promise<TenantCustodyState> {
    await this.pause();
    return projectForTenant(this.state);
  }

  private requireRegistrationAvailable(input: RegistrationInput): void {
    const boundary = boundaryKeyOfAccess(buildAccess(input, undefined));
    const taken = this.state.backends.some(
      (record) =>
        !isPlatformOwned(record) && boundaryKeyOfAccess(record.registration.configuration?.access) === boundary,
    );
    if (taken) throw new KeyFault('BACKEND_ALREADY_REGISTERED');
  }

  async prepareRegistration(input: RegistrationInput): Promise<RegistrationPlan> {
    await this.pause();
    const problems = Object.values(registrationProblems(input));
    if (problems.length > 0) throw new KeyFault('COMMAND_INVALID', problems[0]);
    this.requireRegistrationAvailable(input);
    const id = attemptId(`att_${hexOf(this.next('registration'))}`);
    const generated =
      input.provider === KeyProvider.AWS_KMS
        ? externalId(`trogon-ext-${hexOf(hash(`${id}:a`))}${hexOf(hash(`${id}:b`))}`)
        : undefined;
    const impersonating =
      input.provider === KeyProvider.GOOGLE_CLOUD_KMS && input.impersonatedServiceAccount.trim() !== '';
    const checks = registrationChecks(input.provider, impersonating);
    this.attempts.set(id, {
      kind: 'registration',
      input,
      externalId: generated,
      checks,
      results: new Map(),
      runs: new Map(),
    });
    return { attemptId: id, input, externalId: generated, instructions: instructionsFor(input, generated), checks };
  }

  private requireBindingAvailable(backendId: KeyBackendId, locator: string): void {
    const bound = this.state.keys.some(
      (key) =>
        key.binding.kind === 'customer_managed' &&
        keyBackendIdOf(key.binding.detail.keyBackendId) === backendId &&
        locatorText(key.binding.detail.locator) === locator,
    );
    if (bound) throw new KeyFault('KEY_ALREADY_BOUND');
  }

  async prepareBinding(input: BindingInput): Promise<BindingPlan> {
    await this.pause();
    const parsed = parseKeyBackendId(input.backendId);
    const record = parsed
      ? this.state.backends.find(
          (candidate) => !isPlatformOwned(candidate) && keyBackendIdOf(candidate.registration.keyBackendId) === parsed,
        )
      : undefined;
    const problems = Object.values(bindingProblems(input, record !== undefined));
    if (problems.length > 0 || !record) throw new KeyFault('COMMAND_INVALID', problems[0]);
    this.requireBindable(record);
    const provider = providerOfAccess(record.registration.configuration?.access);
    const problem = locatorProblem(provider, input.locator);
    if (problem) throw new KeyFault('COMMAND_INVALID', problem);
    const locator = buildLocator(provider, input.locator)!;
    const text = locatorText(locator);
    const backendId = keyBackendIdOf(record.registration.keyBackendId);
    this.requireBindingAvailable(backendId, text);
    const id = attemptId(`att_${hexOf(this.next('binding'))}`);
    this.attempts.set(id, {
      kind: 'binding',
      backendId,
      input: { ...input, locator: text },
      checks: bindingChecks,
      results: new Map(),
      runs: new Map(),
    });
    return { attemptId: id, backendId, checks: bindingChecks };
  }

  private requireBindable(record: BackendRecord): void {
    const denial = registrationDenial(record.registration.admission);
    if (denial !== undefined) throw keyUseDenied(denial);
    if (record.registration.lifecycle !== KeyBackendLifecycle.ACTIVE) throw new KeyFault('BACKEND_NOT_ACTIVE');
  }

  async runCheck(id: AttemptId, checkId: string): Promise<CheckResult> {
    await this.pause();
    const attempt = this.attemptOf(id);
    const index = attempt.checks.findIndex((check) => check.id === checkId);
    if (index < 0) throw new KeyFault('CHECK_NOT_FOUND');
    if (attempt.kind === 'binding') this.requireBindable(this.tenantBackend(attempt.backendId));
    const blocked = attempt.checks.slice(0, index).some((check) => attempt.results.get(check.id)?.outcome !== 'pass');
    const priorRuns = attempt.runs.get(checkId) ?? 0;
    let result: CheckResult;
    if (blocked) {
      result = { checkId, outcome: 'skipped', detail: 'Skipped because an earlier check did not pass.' };
    } else {
      attempt.runs.set(checkId, priorRuns + 1);
      result =
        attempt.kind === 'registration'
          ? evaluateRegistrationCheck(attempt.input, checkId, priorRuns)
          : this.bindingCheck(attempt, checkId);
    }
    attempt.results.set(checkId, result);
    return result;
  }

  private bindingCheck(attempt: BindingAttempt, checkId: string): CheckResult {
    const record = this.tenantBackend(attempt.backendId);
    const outage = this.outages.get(attempt.backendId);
    if (checkId === 'access' && outage !== undefined) {
      return {
        checkId,
        outcome: 'fail',
        detail: `The platform could not reach the provider: ${failureReasonLabels[outage]}.`,
        reason: outage,
      };
    }
    const provider = providerOfAccess(record.registration.configuration?.access);
    return evaluateBindingCheck(record, buildLocator(provider, attempt.input.locator)!, checkId);
  }

  private assertPassed(attempt: Attempt): void {
    for (const check of attempt.checks) {
      const result = attempt.results.get(check.id);
      if (!result) throw new KeyFault('VALIDATION_INCOMPLETE');
      if (result.outcome !== 'pass') throw new KeyFault('VALIDATION_FAILED', check.label);
    }
  }

  async completeRegistration(id: AttemptId): Promise<BackendRecord> {
    await this.pause();
    const attempt = this.attemptOf(id);
    if (attempt.kind !== 'registration') throw new KeyFault('ATTEMPT_NOT_FOUND');
    this.assertPassed(attempt);
    this.requireRegistrationAvailable(attempt.input);
    const now = this.clock();
    const input = attempt.input;
    const shortcut =
      input.provider === KeyProvider.AWS_KMS ? 'aws' : input.provider === KeyProvider.GOOGLE_CLOUD_KMS ? 'gcp' : 'obao';
    const backend: BackendRecord = {
      displayName: input.displayName.trim(),
      registration: registrationOf({
        id: keyBackendId(
          `kbe_${shortcut}_${slug(input.displayName) || 'backend'}_${hexOf(this.next(input.displayName)).slice(0, 4)}`,
        ),
        owner: 'tenant',
        access: buildAccess(input, attempt.externalId),
        health: healthOf({ status: 'connected' }, now),
      }),
      registeredAt: now,
      revisions: [{ revision: 1, at: now, summary: 'Registered after every validation check passed.' }],
    };
    this.state = { ...this.state, backends: [...this.state.backends, backend] };
    this.attempts.delete(id);
    return backend;
  }

  async completeBinding(id: AttemptId): Promise<TenantKey> {
    await this.pause();
    const attempt = this.attemptOf(id);
    if (attempt.kind !== 'binding') throw new KeyFault('ATTEMPT_NOT_FOUND');
    this.requireBindable(this.tenantBackend(attempt.backendId));
    this.assertPassed(attempt);
    this.requireBindingAvailable(attempt.backendId, attempt.input.locator);
    const now = this.clock();
    const record = this.tenantBackend(attempt.backendId);
    const provider = providerOfAccess(record.registration.configuration?.access);
    const ref = keyRef(`kr_${hexOf(this.next(attempt.input.locator))}`);
    const observation = stateObservation(KeyState.ACTIVE, now);
    const bound: TenantKey = {
      keyRef: ref,
      purpose: attempt.input.purpose,
      state: observation,
      capabilities:
        attempt.input.capabilities.length > 0 ? attempt.input.capabilities : [KeyCapability.ENVELOPE_WRAPPING],
      boundAt: now,
      usage: emptyUsage(),
      binding: {
        kind: 'customer_managed',
        detail: bindingDetail({
          keyRef: ref,
          backendId: attempt.backendId,
          purpose: attempt.input.purpose,
          locator: buildLocator(provider, attempt.input.locator)!,
          state: observation,
        }),
      },
    };
    this.state = { ...this.state, keys: [...this.state.keys, bound] };
    this.attempts.delete(id);
    return bound;
  }

  private probe(record: BackendRecord, now: Date) {
    const outage = this.outages.get(keyBackendIdOf(record.registration.keyBackendId));
    const expiry = record.registration.health?.trustExpiry;
    const health = healthOf(outage === undefined ? { status: 'connected' } : { status: 'failed', reason: outage }, now);
    if (expiry) health.trustExpiry = expiry;
    return { health, outage };
  }

  async validateBackend(backendId: KeyBackendId): Promise<BackendRecord> {
    await this.pause();
    const record = this.tenantBackend(backendId);
    const denial = registrationDenial(record.registration.admission);
    if (denial !== undefined) throw keyUseDenied(denial, 'The platform makes no call while a registration is paused.');
    const { health } = this.probe(record, this.clock());
    return this.replaceBackend({ ...record, registration: withRegistration(record.registration, { health }) });
  }

  async pauseBackend(backendId: KeyBackendId): Promise<BackendRecord> {
    await this.pause();
    const record = this.tenantBackend(backendId);
    if (record.registration.lifecycle !== KeyBackendLifecycle.ACTIVE) throw new KeyFault('BACKEND_NOT_ACTIVE');
    if (record.registration.admission === KeyBackendAdmission.PAUSED) throw new KeyFault('BACKEND_ALREADY_PAUSED');
    return this.replaceBackend({
      ...record,
      registration: withRegistration(record.registration, { admission: KeyBackendAdmission.PAUSED }),
    });
  }

  async resumeBackend(backendId: KeyBackendId): Promise<BackendRecord> {
    await this.pause();
    const record = this.tenantBackend(backendId);
    if (record.registration.admission !== KeyBackendAdmission.PAUSED) throw new KeyFault('BACKEND_NOT_PAUSED');
    const { health, outage } = this.probe(record, this.clock());
    if (outage !== undefined) {
      this.replaceBackend({ ...record, registration: withRegistration(record.registration, { health }) });
      throw new KeyFault('RESUME_VALIDATION_FAILED', failureReasonLabels[outage]);
    }
    return this.replaceBackend({
      ...record,
      registration: withRegistration(record.registration, { admission: KeyBackendAdmission.ENABLED, health }),
    });
  }

  async setDefaultKey(ref: KeyRef): Promise<void> {
    await this.pause();
    const key = this.keyOf(ref);
    if (this.state.migrations.some((migration) => migration.purpose === key.purpose && isInFlight(migration))) {
      throw new KeyFault('MIGRATION_IN_FLIGHT');
    }
    const current = this.endpointOf(key.purpose);
    if (endpointKeyRef(current) === ref) throw new KeyFault('ALREADY_DEFAULT');
    this.requireUse(ref, 'wrap');
    this.setCustody(key.purpose, { tier: key.binding.kind, keyRef: ref });
  }

  private createDedicatedKey(purpose: PurposeId): TenantKey {
    const existing = this.state.keys.find(
      (key) => key.binding.kind === 'dedicated' && key.purpose === purpose && key.state.state === KeyState.ACTIVE,
    );
    if (existing) return existing;
    const now = this.clock();
    const ref = keyRef(`kr_${hexOf(this.next(`dedicated:${purpose}`))}`);
    const created: TenantKey = {
      keyRef: ref,
      purpose,
      state: create(KeyStateObservationSchema, { state: KeyState.ACTIVE, observedAt: timestampFromDate(now) }),
      capabilities: [KeyCapability.ENVELOPE_WRAPPING],
      boundAt: now,
      usage: emptyUsage(),
      binding: { kind: 'dedicated' },
    };
    const placements = new Map(this.state.placements);
    placements.set(ref, seedBackendIds.platformPrimary);
    this.state = { ...this.state, keys: [...this.state.keys, created], placements };
    return created;
  }

  async startMigration(input: StartMigrationInput): Promise<StartMigrationResult> {
    await this.pause();
    if (!this.state.purposes.some((purpose) => purpose.id === input.purpose)) throw new KeyFault('PURPOSE_NOT_FOUND');
    const from = this.endpointOf(input.purpose);
    if (this.state.migrations.some((m) => m.purpose === input.purpose && isInFlight(m))) {
      throw new KeyFault('MIGRATION_IN_FLIGHT');
    }
    const to = this.resolveTarget(input.purpose, input.target);
    if (sameEndpoint(from, to)) throw new KeyFault('SAME_CUSTODY');

    const fromRef = endpointKeyRef(from);
    const toRef = endpointKeyRef(to);
    const total = this.state.payloads
      .filter((set) => set.purpose === input.purpose && set.keyRef === fromRef)
      .reduce((sum, set) => sum + set.count, 0);

    const newDedicatedKey = input.target.tier === 'dedicated' && !this.state.keys.some((key) => key.keyRef === toRef);
    if (total > 0 && fromRef) this.requireUse(fromRef, 'unwrap');
    if (toRef && !newDedicatedKey) this.requireUse(toRef, 'wrap');

    if (input.target.tier === 'dedicated') this.createDedicatedKey(input.purpose);
    const resolved = this.resolveTarget(input.purpose, input.target);
    const resolvedRef = endpointKeyRef(resolved);
    const custody = this.setCustody(input.purpose, resolved);
    if (total === 0) return { custody, migration: undefined };

    const now = this.clock();
    const started = this.next(`migration:${fromRef}:${resolvedRef}`);
    const migration: Migration = {
      id: migrationId(`mig_${hexOf(started)}`),
      operationId: `op_${hexOf(hash(`op:${started}`))}`,
      purpose: input.purpose,
      from,
      to: resolved,
      status: 'running',
      total,
      rewrapped: 0,
      ratePerMinute: Math.max(1, Math.ceil(total / MIGRATION_TARGET_MINUTES)),
      startedAt: now,
      updatedAt: now,
    };
    this.state = { ...this.state, migrations: [...this.state.migrations, migration] };
    return { custody, migration };
  }

  private resolveTarget(purpose: PurposeId, target: CustodyTarget): CustodyEndpoint {
    switch (target.tier) {
      case 'managed':
        return { tier: 'managed' };
      case 'dedicated': {
        const existing = this.state.keys.find(
          (key) => key.binding.kind === 'dedicated' && key.purpose === purpose && key.state.state === KeyState.ACTIVE,
        );
        return existing
          ? { tier: 'dedicated', keyRef: existing.keyRef }
          : { tier: 'dedicated', keyRef: keyRef('kr_00000000') };
      }
      case 'customer_managed': {
        const key = this.keyOf(target.keyRef);
        if (key.binding.kind !== 'customer_managed') throw new KeyFault('KEY_NOT_FOUND');
        if (key.purpose !== purpose) throw new KeyFault('PURPOSE_MISMATCH');
        return { tier: 'customer_managed', keyRef: target.keyRef };
      }
    }
  }

  private migrationOf(id: MigrationId): Migration {
    const migration = this.state.migrations.find((candidate) => candidate.id === id);
    if (!migration) throw new KeyFault('MIGRATION_NOT_FOUND');
    return migration;
  }

  private replace(migration: Migration): Migration {
    this.state = {
      ...this.state,
      migrations: this.state.migrations.map((candidate) => (candidate.id === migration.id ? migration : candidate)),
    };
    return migration;
  }

  async pauseMigration(id: MigrationId): Promise<Migration> {
    await this.pause();
    const migration = this.migrationOf(id);
    if (migration.status === 'completed') throw new KeyFault('ALREADY_COMPLETED');
    if (migration.status !== 'running') throw new KeyFault('NOT_RUNNING');
    return this.replace({ ...migration, status: 'paused', updatedAt: this.clock() });
  }

  async resumeMigration(id: MigrationId): Promise<Migration> {
    await this.pause();
    const migration = this.migrationOf(id);
    if (migration.status === 'completed') throw new KeyFault('ALREADY_COMPLETED');
    if (migration.status !== 'paused') throw new KeyFault('NOT_PAUSED');
    const denial = this.migrationDenial(migration);
    if (denial !== undefined) throw keyUseDenied(denial);
    return this.replace({ ...migration, status: 'running', updatedAt: this.clock() });
  }
}
