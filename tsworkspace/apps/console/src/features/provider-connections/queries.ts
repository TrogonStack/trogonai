import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { CredentialBindingLifecycle, type BindingReplacementMode } from '@trogonai/proto';

import { useProviderConnectionsClient } from './context';
import { commands } from './domain/commands';
import {
  assembleReplacement,
  buildBinding,
  buildRegistration,
  type BindingDraft,
  type ProfileDraft,
  type RegistrationDraft,
  type RoutingPolicyDraft,
} from './domain/draft';
import { ProviderConnectionFault } from './domain/faults';
import { toConnectionView, type ConnectionView } from './domain/view';

const root = ['provider-connections'] as const;

export const providerConnectionKeys = {
  all: root,
  list: () => [...root, 'list'] as const,
  detail: (id: string) => [...root, 'detail', id] as const,
  history: (id: string) => [...root, 'history', id] as const,
};

export function useConnectionsQuery() {
  const client = useProviderConnectionsClient();
  return useQuery({
    queryKey: providerConnectionKeys.list(),
    queryFn: async () => (await client.list()).map(toConnectionView),
  });
}

export function useConnectionQuery(connectionId: string) {
  const client = useProviderConnectionsClient();
  return useQuery({
    queryKey: providerConnectionKeys.detail(connectionId),
    queryFn: async (): Promise<ConnectionView | null> => {
      const record = await client.get(connectionId);
      return record ? toConnectionView(record) : null;
    },
  });
}

export function useConnectionHistoryQuery(connectionId: string) {
  const client = useProviderConnectionsClient();
  return useQuery({
    queryKey: providerConnectionKeys.history(connectionId),
    queryFn: () => client.history(connectionId),
  });
}

function useInvalidatingMutation<TVariables, TResult>(mutationFn: (variables: TVariables) => Promise<TResult>) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn,
    onSettled: () => queryClient.invalidateQueries({ queryKey: providerConnectionKeys.all }),
  });
}

export interface Targeted {
  readonly connectionId: string;
  readonly expectedRevision: number;
}

export function useRegisterConnection() {
  const client = useProviderConnectionsClient();
  return useInvalidatingMutation(async (draft: RegistrationDraft) => {
    const record = await client.dispatch(commands.register(await buildRegistration(draft)));
    return toConnectionView(record);
  });
}

export function useActivateConnection() {
  const client = useProviderConnectionsClient();
  return useInvalidatingMutation(async ({ connectionId, expectedRevision }: Targeted) => {
    const record = await client.get(connectionId);
    const pending = record?.connection.bindings.find((entry) => entry.lifecycle === CredentialBindingLifecycle.PENDING);
    if (!record || !pending) throw new ProviderConnectionFault('PROVIDER_CONNECTION_NOT_PENDING');
    const validation = await client.requestValidation({ connectionId, binding: pending.binding! });
    return client.dispatch(commands.activate(connectionId, validation), { expectedRevision });
  });
}

export function useReplaceBinding() {
  const client = useProviderConnectionsClient();
  return useInvalidatingMutation(
    async (
      input: Targeted & { previousBindingId: string; replacement: BindingDraft; mode: BindingReplacementMode },
    ) => {
      const replacement = await buildBinding(input.replacement);
      const validation = await client.requestValidation({ connectionId: input.connectionId, binding: replacement });
      return client.dispatch(
        commands.replaceBinding(
          assembleReplacement({
            connectionId: input.connectionId,
            previousBindingId: input.previousBindingId,
            replacement,
            mode: input.mode,
            validation,
          }),
        ),
        { expectedRevision: input.expectedRevision },
      );
    },
  );
}

export function useRevokeBinding() {
  const client = useProviderConnectionsClient();
  return useInvalidatingMutation(({ connectionId, bindingId, expectedRevision }: Targeted & { bindingId: string }) =>
    client.dispatch(commands.revokeBinding(connectionId, bindingId), { expectedRevision }),
  );
}

export function useRevokeConnection() {
  const client = useProviderConnectionsClient();
  return useInvalidatingMutation(({ connectionId, expectedRevision }: Targeted) =>
    client.dispatch(commands.revokeConnection(connectionId), { expectedRevision }),
  );
}

export function useSetProfile() {
  const client = useProviderConnectionsClient();
  return useInvalidatingMutation(({ connectionId, expectedRevision, profile }: Targeted & { profile: ProfileDraft }) =>
    client.dispatch(commands.setProfile(connectionId, profile), { expectedRevision }),
  );
}

export function useSetRoutingPolicy() {
  const client = useProviderConnectionsClient();
  return useInvalidatingMutation(
    ({ connectionId, expectedRevision, policy }: Targeted & { policy: RoutingPolicyDraft }) =>
      client.dispatch(commands.setRoutingPolicy(connectionId, policy), { expectedRevision }),
  );
}

export function useEndPinnedSessions() {
  const client = useProviderConnectionsClient();
  return useInvalidatingMutation(({ connectionId, bindingId }: { connectionId: string; bindingId: string }) =>
    client.endPinnedSessions(connectionId, bindingId),
  );
}
