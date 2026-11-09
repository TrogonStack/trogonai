import type { CredentialEvent, VaultEvent } from '@trogonai/proto';

import type { EnvironmentId } from '@/features/environments/domain/environment-id';
import type { ProjectId } from '@/features/projects/domain/project-id';

import type { CredentialCommand, VaultCommand } from './domain/commands';
import type { CredentialState } from './domain/credential-state';
import type { SecretRefId } from './domain/secret-ref';
import type { VaultState } from './domain/vault';

export interface StoredVaultEvent {
  readonly revision: number;
  readonly event: VaultEvent;
  readonly occurredAt: Date;
  readonly actor: string;
  readonly commandId: string;
}

export interface StoredCredentialEvent {
  readonly revision: number;
  readonly event: CredentialEvent;
  readonly occurredAt: Date;
  readonly actor: string;
  readonly commandId: string;
}

export interface DispatchOptions {
  readonly expectedRevision?: number;
  readonly commandId?: string;
}

export interface RevealedSecret {
  readonly secret: SecretRefId;
  readonly version: number;
  readonly value: string;
}

export interface CredentialDispatchResult {
  readonly credential: CredentialState;
  readonly revealed?: RevealedSecret;
}

export interface EnvironmentDirectory {
  exists(environmentId: EnvironmentId): Promise<boolean>;
}

export interface SecretsClient {
  listVaults(): Promise<readonly VaultState[]>;
  getVault(vaultId: string): Promise<VaultState | undefined>;
  vaultHistory(vaultId: string): Promise<readonly StoredVaultEvent[]>;
  listCredentials(): Promise<readonly CredentialState[]>;
  getCredential(secret: string): Promise<CredentialState | undefined>;
  credentialHistory(secret: string): Promise<readonly StoredCredentialEvent[]>;
  dispatchVault(command: VaultCommand, options?: DispatchOptions): Promise<VaultState>;
  dispatchCredential(command: CredentialCommand, options?: DispatchOptions): Promise<CredentialDispatchResult>;
}

export interface SecretsClients {
  forProject(projectId: ProjectId): SecretsClient;
}
