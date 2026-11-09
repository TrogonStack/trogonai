import {
  AuthorizationScheme,
  CredentialBindingLifecycle,
  OpenAiOutputLimitField,
  ProviderAccessScope,
  ProviderConnectionLifecycle,
  ProviderProtocol,
  ProviderRoutingEligibility,
  type BindingState,
  type CredentialBindingConfiguration,
} from '@trogonai/proto';

import type { ProviderConnectionRecord } from '../client';
import { digestHex } from './digest';
import { authorizationLabels } from './labels';

export type ConnectionId = string & { readonly __brand: 'ConnectionId' };
export type BindingId = string & { readonly __brand: 'BindingId' };

export function asConnectionId(value: string): ConnectionId {
  return value as ConnectionId;
}

export function asBindingId(value: string): BindingId {
  return value as BindingId;
}

export type EndpointView =
  | { readonly kind: 'network'; readonly baseUrl: string }
  | { readonly kind: 'deployment-local'; readonly endpointId: string };

export type AuthenticationView =
  | { readonly kind: 'stored-api-key'; readonly secretRef: string; readonly authorization: AuthorizationScheme }
  | { readonly kind: 'deployment-no-credential' }
  | {
      readonly kind: 'gateway';
      readonly mode: 'configured-key' | 'override-key' | 'passthrough' | 'no-authentication';
      readonly authorization: AuthorizationScheme;
      readonly secretRef?: string;
    };

export interface ModelView {
  readonly identifier: string;
  readonly providerRevision?: string;
  readonly catalogPin?: { readonly modelId: string; readonly version: string; readonly definitionDigestHex: string };
}

export interface BindingView {
  readonly id: BindingId;
  readonly lifecycle: CredentialBindingLifecycle;
  readonly authentication: AuthenticationView;
  readonly digestHex: string;
  readonly pinnedSessions: number;
}

export interface ConnectionView {
  readonly id: ConnectionId;
  readonly revision: number;
  readonly displayName: string;
  readonly description: string;
  readonly parentNodeId: string;
  readonly providerId: string;
  readonly endpoint: EndpointView;
  readonly accessScope: ProviderAccessScope;
  readonly lifecycle: ProviderConnectionLifecycle;
  readonly eligibility: ProviderRoutingEligibility;
  readonly preference: number;
  readonly models: readonly ModelView[];
  readonly protocols: readonly ProviderProtocol[];
  readonly openAiOutputLimit?: OpenAiOutputLimitField;
  readonly bindings: readonly BindingView[];
  readonly activeBinding?: BindingView;
  readonly configurationDigestHex: string;
  readonly configurationByteLength: number;
}

function authenticationView(configuration: CredentialBindingConfiguration | undefined): AuthenticationView {
  const authentication = configuration?.authentication;
  switch (authentication?.case) {
    case 'storedApiKey':
      return {
        kind: 'stored-api-key',
        secretRef: authentication.value.secretRef?.value ?? '',
        authorization: authentication.value.authorization,
      };
    case 'gateway': {
      const gateway = authentication.value;
      switch (gateway.mode.case) {
        case 'configuredKey':
          return {
            kind: 'gateway',
            mode: 'configured-key',
            authorization: gateway.authorization,
            secretRef: gateway.mode.value.secretRef?.value,
          };
        case 'overrideKey':
          return {
            kind: 'gateway',
            mode: 'override-key',
            authorization: gateway.authorization,
            secretRef: gateway.mode.value.secretRef?.value,
          };
        case 'passthrough':
          return { kind: 'gateway', mode: 'passthrough', authorization: gateway.authorization };
        default:
          return { kind: 'gateway', mode: 'no-authentication', authorization: gateway.authorization };
      }
    }
    default:
      return { kind: 'deployment-no-credential' };
  }
}

function bindingView(entry: BindingState, pinned: ReadonlyMap<string, number>): BindingView {
  const id = entry.binding!.id!.value;
  return {
    id: asBindingId(id),
    lifecycle: entry.lifecycle,
    authentication: authenticationView(entry.binding!.configuration),
    digestHex: digestHex(entry.binding!.configurationDigest!),
    pinnedSessions: pinned.get(id) ?? 0,
  };
}

export function toConnectionView(record: ProviderConnectionRecord): ConnectionView {
  const { connection } = record;
  const configuration = connection.configuration!;
  const endpoint = configuration.endpoint!.endpoint;
  const bindings = connection.bindings.map((entry) => bindingView(entry, record.pinnedSessions));
  return {
    id: asConnectionId(connection.id!.value),
    revision: record.revision,
    displayName: connection.profile?.displayName ?? '',
    description: connection.profile?.description ?? '',
    parentNodeId: configuration.parent?.value ?? '',
    providerId: configuration.provider?.value ?? '',
    endpoint:
      endpoint.case === 'deploymentLocal'
        ? { kind: 'deployment-local', endpointId: endpoint.value.endpointId }
        : { kind: 'network', baseUrl: endpoint.value?.baseUrl ?? '' },
    accessScope: configuration.accessScope,
    lifecycle: connection.lifecycle,
    eligibility: connection.routingPolicy!.eligibility,
    preference: connection.routingPolicy!.preference,
    models: configuration.allowedModels.map((model) => ({
      identifier: model.identifier!.value,
      providerRevision: model.providerRevision || undefined,
      catalogPin: model.catalogPin
        ? {
            modelId: model.catalogPin.versionRef!.modelId,
            version: model.catalogPin.versionRef!.version,
            definitionDigestHex: digestHex(model.catalogPin.definitionDigest!),
          }
        : undefined,
    })),
    protocols: configuration.supportedProtocols,
    openAiOutputLimit:
      configuration.openAiOutputLimitField === OpenAiOutputLimitField.UNSPECIFIED
        ? undefined
        : configuration.openAiOutputLimitField,
    bindings,
    activeBinding: bindings.find((binding) => binding.lifecycle === CredentialBindingLifecycle.ACTIVE),
    configurationDigestHex: digestHex(connection.configurationDigest!),
    configurationByteLength: connection.configurationBytes.length,
  };
}

export function describeAuthentication(authentication: AuthenticationView): string {
  switch (authentication.kind) {
    case 'stored-api-key':
      return `Stored API key (${authorizationLabels[authentication.authorization]})`;
    case 'deployment-no-credential':
      return 'No credential (deployment-local)';
    case 'gateway': {
      const scheme =
        authentication.authorization === AuthorizationScheme.UNSPECIFIED
          ? ''
          : ` (${authorizationLabels[authentication.authorization]})`;
      const modes = {
        'configured-key': 'Gateway configured key',
        'override-key': 'Gateway override key',
        passthrough: 'Gateway passthrough',
        'no-authentication': 'Gateway no authentication',
      } as const;
      return `${modes[authentication.mode]}${scheme}`;
    }
  }
}

export function describeEndpoint(endpoint: EndpointView): string {
  return endpoint.kind === 'network' ? endpoint.baseUrl : `local:${endpoint.endpointId}`;
}

export function canActivate(connection: ConnectionView): boolean {
  return connection.lifecycle === ProviderConnectionLifecycle.PENDING;
}

export function isLiveConnection(connection: ConnectionView): boolean {
  return (
    connection.lifecycle === ProviderConnectionLifecycle.PENDING ||
    connection.lifecycle === ProviderConnectionLifecycle.ACTIVE
  );
}

export function isActiveConnection(connection: ConnectionView): boolean {
  return connection.lifecycle === ProviderConnectionLifecycle.ACTIVE;
}
