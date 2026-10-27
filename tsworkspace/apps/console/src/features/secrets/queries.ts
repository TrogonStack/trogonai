import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import type {
  CredentialPolicyReason,
  MaterialVerificationOutcome,
  SecretMaterialKind,
  SecretPurpose,
} from '@trogonai/proto';

import type { EnvironmentId } from '@/features/environments/domain/environment-id';
import { useCurrentProjectId } from '@/features/projects/current-project';
import type { ProjectId } from '@/features/projects/domain/project-id';

import type { CredentialDispatchResult } from './client';
import { useSecretsClient } from './context';
import {
  credentialCommands,
  vaultCommands,
  type MaterialInput,
  type VaultTransition,
  type VerificationSimulation,
} from './domain/commands';
import type { SecretRefId } from './domain/secret-ref';
import { toCredentialView, toVaultView, type CredentialView, type VaultView } from './domain/view';

const root = ['secrets'] as const;

export const secretKeys = {
  all: root,
  vaults: (project: ProjectId | undefined) => [...root, project, 'vaults'] as const,
  vault: (project: ProjectId | undefined, id: string) => [...root, project, 'vault', id] as const,
  credentials: (project: ProjectId | undefined) => [...root, project, 'credentials'] as const,
  credential: (project: ProjectId | undefined, id: string) => [...root, project, 'credential', id] as const,
  vaultHistory: (project: ProjectId | undefined, id: string) => [...root, project, 'vault-history', id] as const,
  credentialHistory: (project: ProjectId | undefined, id: string) =>
    [...root, project, 'credential-history', id] as const,
};

export function useVaultsQuery() {
  const client = useSecretsClient();
  const project = useCurrentProjectId();
  return useQuery({
    enabled: project !== undefined,
    queryKey: secretKeys.vaults(project),
    queryFn: async (): Promise<readonly VaultView[]> => {
      const [vaults, credentials] = await Promise.all([client.listVaults(), client.listCredentials()]);
      return vaults.map((vault) => toVaultView(vault, credentials));
    },
  });
}

export function useVaultQuery(vaultId: string) {
  const client = useSecretsClient();
  const project = useCurrentProjectId();
  return useQuery({
    enabled: project !== undefined,
    queryKey: secretKeys.vault(project, vaultId),
    queryFn: async (): Promise<VaultView | null> => {
      const [vault, credentials] = await Promise.all([client.getVault(vaultId), client.listCredentials()]);
      return vault ? toVaultView(vault, credentials) : null;
    },
  });
}

export function useCredentialsQuery() {
  const client = useSecretsClient();
  const project = useCurrentProjectId();
  return useQuery({
    enabled: project !== undefined,
    queryKey: secretKeys.credentials(project),
    queryFn: async (): Promise<readonly CredentialView[]> => {
      const now = new Date();
      return (await client.listCredentials()).map((credential) => toCredentialView(credential, now));
    },
  });
}

export function useCredentialQuery(secret: string) {
  const client = useSecretsClient();
  const project = useCurrentProjectId();
  return useQuery({
    enabled: project !== undefined,
    queryKey: secretKeys.credential(project, secret),
    queryFn: async (): Promise<CredentialView | null> => {
      const credential = await client.getCredential(secret);
      return credential ? toCredentialView(credential) : null;
    },
  });
}

export function useVaultHistoryQuery(vaultId: string) {
  const client = useSecretsClient();
  const project = useCurrentProjectId();
  return useQuery({
    enabled: project !== undefined,
    queryKey: secretKeys.vaultHistory(project, vaultId),
    queryFn: () => client.vaultHistory(vaultId),
  });
}

export function useCredentialHistoryQuery(secret: string) {
  const client = useSecretsClient();
  const project = useCurrentProjectId();
  return useQuery({
    enabled: project !== undefined,
    queryKey: secretKeys.credentialHistory(project, secret),
    queryFn: () => client.credentialHistory(secret),
  });
}

function useInvalidatingMutation<TVariables, TResult>(mutationFn: (variables: TVariables) => Promise<TResult>) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn,
    onSettled: () => queryClient.invalidateQueries({ queryKey: secretKeys.all }),
  });
}

export interface VaultTarget {
  readonly vaultId: string;
  readonly expectedRevision: number;
}

export interface CredentialTarget {
  readonly secret: SecretRefId;
  readonly expectedRevision: number;
}

export function useCreateVault() {
  const client = useSecretsClient();
  return useInvalidatingMutation((input: { displayName: string; environment: EnvironmentId }) =>
    client.dispatchVault(vaultCommands.create(input)),
  );
}

export function useRenameVault() {
  const client = useSecretsClient();
  return useInvalidatingMutation(({ vaultId, expectedRevision, displayName }: VaultTarget & { displayName: string }) =>
    client.dispatchVault(vaultCommands.rename(vaultId, displayName), { expectedRevision }),
  );
}

export function useVaultTransition() {
  const client = useSecretsClient();
  return useInvalidatingMutation(
    ({ vaultId, expectedRevision, transition }: VaultTarget & { transition: VaultTransition }) =>
      client.dispatchVault(vaultCommands.transition(vaultId, transition), { expectedRevision }),
  );
}

export interface CreateCredentialInput {
  readonly vaultId: string;
  readonly displayName: string;
  readonly purpose: SecretPurpose;
  readonly kind: SecretMaterialKind;
  readonly material: MaterialInput;
  readonly simulation: VerificationSimulation;
}

export function useCreateCredential() {
  const client = useSecretsClient();
  return useInvalidatingMutation(
    (input: CreateCredentialInput): Promise<CredentialDispatchResult> =>
      client.dispatchCredential(credentialCommands.create(input)),
  );
}

export function useRotateCredential() {
  const client = useSecretsClient();
  return useInvalidatingMutation(
    ({
      secret,
      expectedRevision,
      ...input
    }: CredentialTarget & { material: MaterialInput; overlapHours?: number; simulation: VerificationSimulation }) =>
      client.dispatchCredential(credentialCommands.rotate(secret, input), { expectedRevision }),
  );
}

export function useCompleteVerification() {
  const client = useSecretsClient();
  return useInvalidatingMutation(
    ({ secret, expectedRevision, outcome }: CredentialTarget & { outcome: MaterialVerificationOutcome }) =>
      client.dispatchCredential(credentialCommands.completeVerification(secret, outcome), { expectedRevision }),
  );
}

export function useDisableCredential() {
  const client = useSecretsClient();
  return useInvalidatingMutation(({ secret, expectedRevision }: CredentialTarget) =>
    client.dispatchCredential(credentialCommands.disable(secret), { expectedRevision }),
  );
}

export function useEnableCredential() {
  const client = useSecretsClient();
  return useInvalidatingMutation(({ secret, expectedRevision }: CredentialTarget) =>
    client.dispatchCredential(credentialCommands.enable(secret), { expectedRevision }),
  );
}

export function useRevokeCredential() {
  const client = useSecretsClient();
  return useInvalidatingMutation(
    ({ secret, expectedRevision, reason }: CredentialTarget & { reason: CredentialPolicyReason }) =>
      client.dispatchCredential(credentialCommands.revoke(secret, reason), { expectedRevision }),
  );
}

export function useEndOverlap() {
  const client = useSecretsClient();
  return useInvalidatingMutation(({ secret, expectedRevision }: CredentialTarget) =>
    client.dispatchCredential(credentialCommands.endWebhookOverlap(secret), { expectedRevision }),
  );
}

export function useScheduleDestruction() {
  const client = useSecretsClient();
  return useInvalidatingMutation(
    ({ secret, expectedRevision, executeAfter }: CredentialTarget & { executeAfter: Date }) =>
      client.dispatchCredential(credentialCommands.scheduleDestruction(secret, executeAfter), { expectedRevision }),
  );
}

export function useConfirmDestruction() {
  const client = useSecretsClient();
  return useInvalidatingMutation(({ secret, expectedRevision }: CredentialTarget) =>
    client.dispatchCredential(credentialCommands.confirmDestruction(secret), { expectedRevision }),
  );
}

export function useRenameCredential() {
  const client = useSecretsClient();
  return useInvalidatingMutation(
    ({ secret, expectedRevision, displayName }: CredentialTarget & { displayName: string }) =>
      client.dispatchCredential(credentialCommands.rename(secret, displayName), { expectedRevision }),
  );
}
