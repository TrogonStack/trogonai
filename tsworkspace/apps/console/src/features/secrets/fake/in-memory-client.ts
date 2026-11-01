import { timestampDate } from '@bufbuild/protobuf/wkt';
import { CredentialPolicyReason, type CredentialEvent, type VaultEvent } from '@trogonai/proto';

import type { CustodyTier } from '@/features/custody/domain/custody';
import type { EnvironmentId } from '@/features/environments/domain/environment-id';

import type {
  CredentialDispatchResult,
  DispatchOptions,
  EnvironmentDirectory,
  RevealedSecret,
  SecretsClient,
  StoredCredentialEvent,
  StoredVaultEvent,
} from '../client';
import { credentialCommands, type CredentialCommand, type VaultCommand } from '../domain/commands';
import type { DecisionContext } from '../domain/context';
import { decideCredential } from '../domain/credential';
import { foldCredential, isLive, type CredentialState } from '../domain/credential-state';
import { SecretFault } from '../domain/faults';
import { parseSecretRefId, mintWebhookSharedSecret } from '../domain/secret-ref';
import { decideVault, foldVault, type VaultState } from '../domain/vault';

export const DEMO_PROJECT = 'proj-demo';
const ACTOR = 'console-operator';

export interface InMemoryClientOptions {
  readonly latencyMs?: number;
  readonly clock?: () => Date;
  readonly projectId?: string;
  readonly environments?: EnvironmentDirectory;
}

export interface SeedWriter {
  vault(command: VaultCommand, at: Date): Promise<VaultState>;
  custody(vaultId: string, tier: CustodyTier): void;
  credential(command: CredentialCommand, at: Date): Promise<CredentialDispatchResult>;
}

function sleep(milliseconds: number): Promise<void> {
  return milliseconds > 0 ? new Promise((resolve) => setTimeout(resolve, milliseconds)) : Promise.resolve();
}

function newId(prefix: string): string {
  return `${prefix}_${crypto.randomUUID().replaceAll('-', '').slice(0, 12)}`;
}

function vaultKey(command: VaultCommand): string {
  return command.vaultId;
}

function credentialKey(command: CredentialCommand): string {
  return command.secret;
}

export class InMemorySecretsClient implements SecretsClient {
  private readonly vaultStreams = new Map<string, StoredVaultEvent[]>();
  private readonly credentialStreams = new Map<string, StoredCredentialEvent[]>();
  private readonly vaultCustody = new Map<string, CustodyTier>();
  private readonly latencyMs: number;
  private readonly clock: () => Date;
  private readonly projectId: string;
  private readonly environments: EnvironmentDirectory | undefined;
  private seed: () => Promise<void> = () => Promise.resolve();
  private ready: Promise<void> | undefined;

  constructor(options: InMemoryClientOptions = {}) {
    this.latencyMs = options.latencyMs ?? 0;
    this.clock = options.clock ?? (() => new Date());
    this.projectId = options.projectId ?? DEMO_PROJECT;
    this.environments = options.environments;
  }

  seedWith(populate: (writer: SeedWriter) => Promise<void>): void {
    this.seed = () =>
      populate({
        vault: (command, at) => this.executeVault(command, {}, at, true),
        custody: (vaultId, tier) => void this.vaultCustody.set(vaultId, tier),
        credential: (command, at) => this.executeCredential(command, {}, at, true),
      });
  }

  private async pause(): Promise<void> {
    this.ready ??= this.seed();
    await this.ready;
    await sleep(this.latencyMs);
    this.reapExpiredOverlaps();
  }

  async listVaults(): Promise<readonly VaultState[]> {
    await this.pause();
    return [...this.vaultStreams.keys()].map((id) => this.vaultState(id)).filter((vault) => vault !== undefined);
  }

  async getVault(vaultId: string): Promise<VaultState | undefined> {
    await this.pause();
    return this.vaultState(vaultId);
  }

  async vaultHistory(vaultId: string): Promise<readonly StoredVaultEvent[]> {
    await this.pause();
    return [...(this.vaultStreams.get(vaultId) ?? [])];
  }

  async listCredentials(): Promise<readonly CredentialState[]> {
    await this.pause();
    return [...this.credentialStreams.keys()]
      .map((id) => this.credentialState(id))
      .filter((credential) => credential !== undefined);
  }

  async getCredential(secret: string): Promise<CredentialState | undefined> {
    await this.pause();
    return this.credentialState(secret);
  }

  async credentialHistory(secret: string): Promise<readonly StoredCredentialEvent[]> {
    await this.pause();
    return [...(this.credentialStreams.get(secret) ?? [])];
  }

  async dispatchVault(command: VaultCommand, options: DispatchOptions = {}): Promise<VaultState> {
    await this.pause();
    if (command.case === 'createVault') await this.requireEnvironment(command.environment);
    return this.executeVault(command, options);
  }

  async dispatchCredential(
    command: CredentialCommand,
    options: DispatchOptions = {},
  ): Promise<CredentialDispatchResult> {
    await this.pause();
    return this.executeCredential(command, options);
  }

  private async requireEnvironment(environment: EnvironmentId): Promise<void> {
    if (this.environments && !(await this.environments.exists(environment))) {
      throw new SecretFault('ENVIRONMENT_NOT_FOUND', `No environment has the id ${environment}.`);
    }
  }

  private async executeVault(
    command: VaultCommand,
    options: DispatchOptions,
    at?: Date,
    trusted = false,
  ): Promise<VaultState> {
    const key = vaultKey(command);
    const stream = this.vaultStreams.get(key) ?? [];
    const commandId = options.commandId ?? crypto.randomUUID();
    if (stream.some((stored) => stored.commandId === commandId)) return this.vaultState(key)!;
    this.checkRevision(command.case === 'createVault', stream.length, options, trusted);

    const now = at ?? this.clock();
    const decision = decideVault(this.vaultState(key), command, this.contextAt(now));
    if (!decision.ok) throw decision.fault;
    this.vaultStreams.set(key, [...stream, ...decision.events.map((event) => this.storeVault(event, commandId))]);
    return this.vaultState(key)!;
  }

  private async executeCredential(
    command: CredentialCommand,
    options: DispatchOptions,
    at?: Date,
    trusted = false,
  ): Promise<CredentialDispatchResult> {
    const key = credentialKey(command);
    const stream = this.credentialStreams.get(key) ?? [];
    const commandId = options.commandId ?? crypto.randomUUID();
    if (stream.some((stored) => stored.commandId === commandId)) return { credential: this.credentialState(key)! };
    this.checkRevision(command.case === 'createCredential', stream.length, options, trusted);

    const now = at ?? this.clock();
    const decision = decideCredential(this.credentialState(key), command, this.contextAt(now));
    if (!decision.ok) throw decision.fault;
    this.credentialStreams.set(key, [
      ...stream,
      ...decision.events.map((event) => this.storeCredential(event, commandId)),
    ]);
    const credential = this.credentialState(key)!;
    return { credential, revealed: this.reveal(command, credential) };
  }

  private reveal(command: CredentialCommand, credential: CredentialState): RevealedSecret | undefined {
    const generated =
      (command.case === 'createCredential' || command.case === 'rotateCredential') &&
      command.material.case === 'generated';
    if (!generated || credential.write?.status !== 'ACTIVATED' || credential.activeVersion === undefined) {
      return undefined;
    }
    return { secret: credential.secret, version: credential.activeVersion, value: mintWebhookSharedSecret() };
  }

  private checkRevision(creating: boolean, revision: number, options: DispatchOptions, trusted: boolean): void {
    if (creating || trusted) return;
    if (revision === 0) throw new SecretFault('NOT_FOUND');
    if (options.expectedRevision === undefined || options.expectedRevision < 1) {
      throw new SecretFault('COMMAND_INVALID', 'Commands on an existing resource require an expected revision.');
    }
    if (options.expectedRevision !== revision) {
      throw new SecretFault(
        'REVISION_CONFLICT',
        `The resource is at revision ${revision}, not ${options.expectedRevision}.`,
      );
    }
  }

  private storeVault(event: VaultEvent, commandId: string): StoredVaultEvent {
    return {
      revision: Number(event.revision),
      event,
      occurredAt: event.occurredAt ? timestampDate(event.occurredAt) : new Date(),
      actor: ACTOR,
      commandId,
    };
  }

  private storeCredential(event: CredentialEvent, commandId: string): StoredCredentialEvent {
    return {
      revision: Number(event.revision),
      event,
      occurredAt: event.occurredAt ? timestampDate(event.occurredAt) : new Date(),
      actor: ACTOR,
      commandId,
    };
  }

  private vaultState(vaultId: string): VaultState | undefined {
    const stream = this.vaultStreams.get(vaultId);
    const state = stream ? foldVault(stream.map((stored) => stored.event)) : undefined;
    return state && { ...state, custody: this.vaultCustody.get(vaultId) ?? state.custody };
  }

  private credentialState(secret: string): CredentialState | undefined {
    if (!parseSecretRefId(secret)) return undefined;
    const stream = this.credentialStreams.get(secret);
    return stream ? foldCredential(stream.map((stored) => stored.event)) : undefined;
  }

  private contextAt(now: Date): DecisionContext {
    return {
      now,
      projectId: this.projectId,
      newId,
      vault: (vaultId) => {
        const vault = this.vaultState(vaultId);
        return vault && { lifecycle: vault.lifecycle, environment: vault.environment };
      },
      liveCredentialCount: (vaultId) =>
        [...this.credentialStreams.keys()]
          .map((secret) => this.credentialState(secret))
          .filter((credential) => credential && credential.vaultId === vaultId && isLive(credential)).length,
    };
  }

  private reapExpiredOverlaps(): void {
    const now = this.clock();
    for (const secret of this.credentialStreams.keys()) {
      const credential = this.credentialState(secret);
      if (!credential?.overlap || credential.overlap.expiresAt > now) continue;
      const command = credentialCommands.endWebhookOverlap(credential.secret, CredentialPolicyReason.OVERLAP_EXPIRED);
      const decision = decideCredential(credential, command, this.contextAt(now));
      if (!decision.ok) continue;
      const stream = this.credentialStreams.get(secret) ?? [];
      this.credentialStreams.set(secret, [
        ...stream,
        ...decision.events.map((event) => this.storeCredential(event, crypto.randomUUID())),
      ]);
    }
  }
}
