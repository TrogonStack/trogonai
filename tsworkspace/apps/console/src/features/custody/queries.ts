import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';

import type { StartMigrationInput } from './client';
import { useTenantCustodyClient } from './context';
import type { AttemptId, KeyBackendId, KeyRef, MigrationId } from './domain/identifiers';
import type { BindingInput, RegistrationInput } from './domain/registration';
import { toCustodyView, type CustodyView } from './domain/view';

const root = ['key-custody'] as const;
const MIGRATION_POLL_MS = 1500;

export const custodyKeys = {
  all: root,
  state: [...root, 'state'] as const,
};

export function useCustodyQuery() {
  const client = useTenantCustodyClient();
  return useQuery({
    queryKey: custodyKeys.state,
    queryFn: async (): Promise<CustodyView> => toCustodyView(await client.load()),
    refetchInterval: (query) => (query.state.data?.stats.migrationsInFlight ? MIGRATION_POLL_MS : false),
  });
}

export function useKeyQuery(ref: string) {
  const query = useCustodyQuery();
  return { ...query, data: query.data ? (query.data.keys.find((key) => key.keyRef === ref) ?? null) : undefined };
}

export function useBackendQuery(id: string) {
  const query = useCustodyQuery();
  return {
    ...query,
    data: query.data ? (query.data.backends.find((backend) => backend.id === id) ?? null) : undefined,
  };
}

export function useMigrationQuery(id: string) {
  const query = useCustodyQuery();
  return {
    ...query,
    data: query.data ? (query.data.migrations.find((view) => view.migration.id === id) ?? null) : undefined,
  };
}

function useInvalidatingMutation<TVariables, TResult>(mutationFn: (variables: TVariables) => Promise<TResult>) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn,
    onSettled: () => {
      void queryClient.invalidateQueries({ queryKey: custodyKeys.all });
    },
  });
}

export function usePrepareRegistration() {
  const client = useTenantCustodyClient();
  return useMutation({ mutationFn: (input: RegistrationInput) => client.prepareRegistration(input) });
}

export function usePrepareBinding() {
  const client = useTenantCustodyClient();
  return useMutation({ mutationFn: (input: BindingInput) => client.prepareBinding(input) });
}

export function useRunCheck() {
  const client = useTenantCustodyClient();
  return useMutation({
    mutationFn: ({ attemptId, checkId }: { attemptId: AttemptId; checkId: string }) =>
      client.runCheck(attemptId, checkId),
  });
}

export function useCompleteRegistration() {
  const client = useTenantCustodyClient();
  return useInvalidatingMutation((attemptId: AttemptId) => client.completeRegistration(attemptId));
}

export function useCompleteBinding() {
  const client = useTenantCustodyClient();
  return useInvalidatingMutation((attemptId: AttemptId) => client.completeBinding(attemptId));
}

export function useValidateBackend() {
  const client = useTenantCustodyClient();
  return useInvalidatingMutation((backendId: KeyBackendId) => client.validateBackend(backendId));
}

export function usePauseBackend() {
  const client = useTenantCustodyClient();
  return useInvalidatingMutation((backendId: KeyBackendId) => client.pauseBackend(backendId));
}

export function useResumeBackend() {
  const client = useTenantCustodyClient();
  return useInvalidatingMutation((backendId: KeyBackendId) => client.resumeBackend(backendId));
}

export function useSetDefaultKey() {
  const client = useTenantCustodyClient();
  return useInvalidatingMutation((ref: KeyRef) => client.setDefaultKey(ref));
}

export function useStartMigration() {
  const client = useTenantCustodyClient();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async (input: StartMigrationInput) => {
      const result = await client.startMigration(input);
      await queryClient.cancelQueries({ queryKey: custodyKeys.state });
      queryClient.setQueryData(custodyKeys.state, toCustodyView(await client.load()));
      return result;
    },
    onError: () => queryClient.invalidateQueries({ queryKey: custodyKeys.state }),
  });
}

export function usePauseMigration() {
  const client = useTenantCustodyClient();
  return useInvalidatingMutation((id: MigrationId) => client.pauseMigration(id));
}

export function useResumeMigration() {
  const client = useTenantCustodyClient();
  return useInvalidatingMutation((id: MigrationId) => client.resumeMigration(id));
}
