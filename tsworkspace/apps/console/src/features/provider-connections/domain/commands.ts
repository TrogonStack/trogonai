import { create } from '@bufbuild/protobuf';
import {
  ActivateProviderConnectionSchema,
  CredentialBindingIdSchema,
  ProviderConnectionIdSchema,
  ProviderConnectionProfileSchema,
  ProviderRoutingPolicySchema,
  RevokeCredentialBindingSchema,
  RevokeProviderConnectionSchema,
  SetProviderConnectionProfileSchema,
  SetProviderRoutingPolicySchema,
  type ProviderValidationRef,
  type RegisterProviderConnection,
  type ReplaceCredentialBinding,
} from '@trogonai/proto';

import type { ProviderCommand } from './decider';
import type { ProfileDraft, RoutingPolicyDraft } from './draft';

function connectionId(value: string) {
  return create(ProviderConnectionIdSchema, { value });
}

export const commands = {
  register: (value: RegisterProviderConnection): ProviderCommand => ({ case: 'register', value }),
  replaceBinding: (value: ReplaceCredentialBinding): ProviderCommand => ({ case: 'replaceBinding', value }),
  activate: (id: string, validation: ProviderValidationRef): ProviderCommand => ({
    case: 'activate',
    value: create(ActivateProviderConnectionSchema, { connectionId: connectionId(id), validation }),
  }),
  revokeBinding: (id: string, bindingId: string): ProviderCommand => ({
    case: 'revokeBinding',
    value: create(RevokeCredentialBindingSchema, {
      connectionId: connectionId(id),
      bindingId: create(CredentialBindingIdSchema, { value: bindingId }),
    }),
  }),
  revokeConnection: (id: string): ProviderCommand => ({
    case: 'revokeConnection',
    value: create(RevokeProviderConnectionSchema, { connectionId: connectionId(id) }),
  }),
  setProfile: (id: string, profile: ProfileDraft): ProviderCommand => ({
    case: 'setProfile',
    value: create(SetProviderConnectionProfileSchema, {
      connectionId: connectionId(id),
      profile: create(ProviderConnectionProfileSchema, profile),
    }),
  }),
  setRoutingPolicy: (id: string, policy: RoutingPolicyDraft): ProviderCommand => ({
    case: 'setRoutingPolicy',
    value: create(SetProviderRoutingPolicySchema, {
      connectionId: connectionId(id),
      routingPolicy: create(ProviderRoutingPolicySchema, policy),
    }),
  }),
};
