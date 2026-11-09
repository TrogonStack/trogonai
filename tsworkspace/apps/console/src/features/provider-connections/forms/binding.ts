import { AuthorizationScheme, ProviderAccessScope } from '@trogonai/proto';
import { z } from 'zod';

import type { AuthenticationDraft, BindingDraft } from '../domain/draft';

export const authenticationKinds = ['stored-api-key', 'deployment-no-credential', 'gateway'] as const;
export const gatewayModes = ['configured-key', 'override-key', 'passthrough', 'no-authentication'] as const;

export const bindingFieldsSchema = z.object({
  bindingId: z.string(),
  authenticationKind: z.enum(authenticationKinds),
  secretRef: z.string(),
  authorization: z.number(),
  gatewayMode: z.enum(gatewayModes),
});

export type BindingFormValues = z.infer<typeof bindingFieldsSchema>;

export type EndpointKind = 'network' | 'deployment-local';

export interface BindingScopeContext {
  readonly accessScope: ProviderAccessScope;
  readonly endpointKind: EndpointKind;
}

export const emptyBindingValues: BindingFormValues = {
  bindingId: '',
  authenticationKind: 'stored-api-key',
  secretRef: '',
  authorization: AuthorizationScheme.BEARER_TOKEN,
  gatewayMode: 'configured-key',
};

export type AddIssue = (path: (string | number)[], message: string) => void;

const SECRET_REF_PATTERN = /^secret:\/\/\S+$/;

export function checkBindingValues(
  values: BindingFormValues,
  context: BindingScopeContext,
  add: AddIssue,
  prefix: (string | number)[],
): void {
  const at = (field: keyof BindingFormValues) => [...prefix, field];

  if (!values.bindingId.trim()) add(at('bindingId'), 'Binding id is required.');

  const requiresSecret =
    values.authenticationKind === 'stored-api-key' ||
    (values.authenticationKind === 'gateway' && values.gatewayMode === 'override-key');
  const optionalSecret = values.authenticationKind === 'gateway' && values.gatewayMode === 'configured-key';
  if (requiresSecret && !values.secretRef.trim()) add(at('secretRef'), 'Secret reference is required.');
  if (
    (requiresSecret || optionalSecret) &&
    values.secretRef.trim() &&
    !SECRET_REF_PATTERN.test(values.secretRef.trim())
  ) {
    add(at('secretRef'), 'Use an opaque reference such as secret://providers/name/key. Never paste a credential.');
  }

  if (values.authenticationKind === 'stored-api-key' && values.authorization === AuthorizationScheme.UNSPECIFIED) {
    add(at('authorization'), 'Authorization scheme is required for stored API keys.');
  }

  if (values.authenticationKind === 'gateway') {
    const keyMode = values.gatewayMode === 'configured-key' || values.gatewayMode === 'override-key';
    if (keyMode && values.authorization === AuthorizationScheme.UNSPECIFIED) {
      add(at('authorization'), 'Configured and override key modes require an authorization scheme.');
    }
  }

  if (context.accessScope === ProviderAccessScope.MANAGED) {
    if (values.authenticationKind === 'gateway') {
      add(at('authenticationKind'), 'Managed scope does not accept gateway authentication.');
    }
    if (values.authenticationKind === 'deployment-no-credential' && context.endpointKind !== 'deployment-local') {
      add(at('authenticationKind'), 'No credential is only valid for a deployment-local endpoint.');
    }
  } else if (values.authenticationKind !== 'gateway') {
    add(at('authenticationKind'), 'Gateway-only scope requires gateway authentication.');
  }
}

export function toBindingDraft(values: BindingFormValues): BindingDraft {
  return { bindingId: values.bindingId.trim(), authentication: toAuthenticationDraft(values) };
}

function toAuthenticationDraft(values: BindingFormValues): AuthenticationDraft {
  const secretRef = values.secretRef.trim();
  switch (values.authenticationKind) {
    case 'stored-api-key':
      return { kind: 'stored-api-key', secretRef, authorization: values.authorization };
    case 'deployment-no-credential':
      return { kind: 'deployment-no-credential' };
    case 'gateway': {
      const authorization =
        values.authorization === AuthorizationScheme.UNSPECIFIED
          ? undefined
          : (values.authorization as AuthorizationScheme);
      switch (values.gatewayMode) {
        case 'configured-key':
          return {
            kind: 'gateway',
            authorization,
            mode: { kind: 'configured-key', secretRef: secretRef || undefined },
          };
        case 'override-key':
          return { kind: 'gateway', authorization, mode: { kind: 'override-key', secretRef } };
        case 'passthrough':
          return { kind: 'gateway', authorization, mode: { kind: 'passthrough' } };
        case 'no-authentication':
          return { kind: 'gateway', authorization, mode: { kind: 'no-authentication' } };
      }
    }
  }
}
