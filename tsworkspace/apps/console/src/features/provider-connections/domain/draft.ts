import { create } from '@bufbuild/protobuf';
import {
  AuthorizationScheme,
  BindingReplacementMode,
  CredentialBindingConfigurationSchema,
  CredentialBindingIdSchema,
  CredentialBindingSchema,
  DigestSchema,
  GatewayAuthenticationSchema,
  ModelVersionRefSchema,
  NodeIdSchema,
  OpenAiOutputLimitField,
  ProviderAccessScope,
  ProviderConnectionConfigurationSchema,
  ProviderConnectionIdSchema,
  ProviderConnectionProfileSchema,
  ProviderConnectionRegistrationSchema,
  ProviderEndpointSchema,
  ProviderIdSchema,
  ProviderModelIdentifierSchema,
  ProviderModelOfferingSchema,
  ProviderProtocol,
  ProviderRoutingEligibility,
  ProviderRoutingPolicySchema,
  RegisterProviderConnectionSchema,
  ReplaceCredentialBindingSchema,
  SecretRefSchema,
  type CredentialBinding,
  type CredentialBindingConfiguration,
  type Digest,
  type ProviderConnectionConfiguration,
  type ProviderValidationRef,
  type RegisterProviderConnection,
  type ReplaceCredentialBinding,
} from '@trogonai/proto';

import { commitBindingConfiguration, commitConnectionConfiguration } from './digest';
import { CONTRACT_VERSION } from './validation';

export type EndpointDraft =
  | { readonly kind: 'network'; readonly baseUrl: string }
  | { readonly kind: 'deployment-local'; readonly endpointId: string };

export interface ModelDraft {
  readonly identifier: string;
  readonly providerRevision?: string;
  readonly catalogPin?: {
    readonly modelId: string;
    readonly version: string;
    readonly definitionDigestHex: string;
  };
}

export type GatewayModeDraft =
  | { readonly kind: 'configured-key'; readonly secretRef?: string }
  | { readonly kind: 'override-key'; readonly secretRef: string }
  | { readonly kind: 'passthrough' }
  | { readonly kind: 'no-authentication' };

export type AuthenticationDraft =
  | { readonly kind: 'stored-api-key'; readonly secretRef: string; readonly authorization: AuthorizationScheme }
  | { readonly kind: 'deployment-no-credential' }
  | { readonly kind: 'gateway'; readonly authorization?: AuthorizationScheme; readonly mode: GatewayModeDraft };

export interface BindingDraft {
  readonly bindingId: string;
  readonly authentication: AuthenticationDraft;
}

export interface RoutingPolicyDraft {
  readonly preference: number;
  readonly eligibility: ProviderRoutingEligibility;
}

export interface ProfileDraft {
  readonly displayName: string;
  readonly description: string;
}

export interface RegistrationDraft {
  readonly connectionId: string;
  readonly parentNodeId: string;
  readonly providerId: string;
  readonly endpoint: EndpointDraft;
  readonly accessScope: ProviderAccessScope;
  readonly models: readonly ModelDraft[];
  readonly protocols: readonly ProviderProtocol[];
  readonly openAiOutputLimit?: OpenAiOutputLimitField;
  readonly binding: BindingDraft;
  readonly profile: ProfileDraft;
  readonly routingPolicy: RoutingPolicyDraft;
}

export function hexToDigest(hex: string): Digest {
  const bytes = new Uint8Array(hex.length / 2);
  for (let index = 0; index < bytes.length; index += 1) {
    bytes[index] = Number.parseInt(hex.slice(index * 2, index * 2 + 2), 16);
  }
  return create(DigestSchema, { algorithm: 'sha256', value: bytes });
}

export function authenticationConfiguration(authentication: AuthenticationDraft): CredentialBindingConfiguration {
  switch (authentication.kind) {
    case 'stored-api-key':
      return create(CredentialBindingConfigurationSchema, {
        contractVersion: CONTRACT_VERSION,
        authentication: {
          case: 'storedApiKey',
          value: {
            secretRef: create(SecretRefSchema, { value: authentication.secretRef }),
            authorization: authentication.authorization,
          },
        },
      });
    case 'deployment-no-credential':
      return create(CredentialBindingConfigurationSchema, {
        contractVersion: CONTRACT_VERSION,
        authentication: { case: 'deploymentNoCredential', value: {} },
      });
    case 'gateway': {
      const gateway = create(GatewayAuthenticationSchema, {
        authorization: authentication.authorization ?? AuthorizationScheme.UNSPECIFIED,
      });
      const mode = authentication.mode;
      switch (mode.kind) {
        case 'configured-key':
          gateway.mode = {
            case: 'configuredKey',
            value: {
              $typeName: 'trogonai.model_access.provider_connections.v1alpha1.GatewayConfiguredKey',
              secretRef: mode.secretRef ? create(SecretRefSchema, { value: mode.secretRef }) : undefined,
            },
          };
          break;
        case 'override-key':
          gateway.mode = {
            case: 'overrideKey',
            value: {
              $typeName: 'trogonai.model_access.provider_connections.v1alpha1.GatewayOverrideKey',
              secretRef: create(SecretRefSchema, { value: mode.secretRef }),
            },
          };
          break;
        case 'passthrough':
          gateway.mode = { case: 'passthrough', value: { $typeName: 'google.protobuf.Empty' } };
          break;
        case 'no-authentication':
          gateway.mode = { case: 'noAuthentication', value: { $typeName: 'google.protobuf.Empty' } };
          break;
      }
      return create(CredentialBindingConfigurationSchema, {
        contractVersion: CONTRACT_VERSION,
        authentication: { case: 'gateway', value: gateway },
      });
    }
  }
}

export async function buildBinding(draft: BindingDraft): Promise<CredentialBinding> {
  const configuration = authenticationConfiguration(draft.authentication);
  const committed = await commitBindingConfiguration(configuration);
  return create(CredentialBindingSchema, {
    id: create(CredentialBindingIdSchema, { value: draft.bindingId }),
    configuration,
    configurationDigest: committed.digest,
    configurationBytes: committed.bytes,
  });
}

export function connectionConfiguration(draft: RegistrationDraft): ProviderConnectionConfiguration {
  return create(ProviderConnectionConfigurationSchema, {
    parent: create(NodeIdSchema, { value: draft.parentNodeId }),
    provider: create(ProviderIdSchema, { value: draft.providerId }),
    endpoint: create(ProviderEndpointSchema, {
      endpoint:
        draft.endpoint.kind === 'network'
          ? {
              case: 'network',
              value: {
                $typeName: 'trogonai.model_access.provider_connections.v1alpha1.NetworkEndpoint',
                baseUrl: draft.endpoint.baseUrl,
              },
            }
          : {
              case: 'deploymentLocal',
              value: {
                $typeName: 'trogonai.model_access.provider_connections.v1alpha1.DeploymentLocalEndpoint',
                endpointId: draft.endpoint.endpointId,
              },
            },
    }),
    accessScope: draft.accessScope,
    allowedModels: draft.models.map((model) =>
      create(ProviderModelOfferingSchema, {
        identifier: create(ProviderModelIdentifierSchema, { value: model.identifier }),
        providerRevision: model.providerRevision ?? '',
        catalogPin: model.catalogPin
          ? {
              versionRef: create(ModelVersionRefSchema, {
                modelId: model.catalogPin.modelId,
                version: model.catalogPin.version,
              }),
              definitionDigest: hexToDigest(model.catalogPin.definitionDigestHex),
            }
          : undefined,
      }),
    ),
    supportedProtocols: [...draft.protocols],
    openAiOutputLimitField: draft.openAiOutputLimit ?? OpenAiOutputLimitField.UNSPECIFIED,
    contractVersion: CONTRACT_VERSION,
  });
}

export async function buildRegistration(draft: RegistrationDraft): Promise<RegisterProviderConnection> {
  const configuration = connectionConfiguration(draft);
  const committed = await commitConnectionConfiguration(configuration);
  return create(RegisterProviderConnectionSchema, {
    registration: create(ProviderConnectionRegistrationSchema, {
      id: create(ProviderConnectionIdSchema, { value: draft.connectionId }),
      configuration,
      configurationDigest: committed.digest,
      configurationBytes: committed.bytes,
      initialBinding: await buildBinding(draft.binding),
      profile: create(ProviderConnectionProfileSchema, draft.profile),
      routingPolicy: create(ProviderRoutingPolicySchema, draft.routingPolicy),
    }),
  });
}

export function assembleReplacement(input: {
  connectionId: string;
  previousBindingId: string;
  replacement: CredentialBinding;
  mode: BindingReplacementMode;
  validation: ProviderValidationRef;
}): ReplaceCredentialBinding {
  return create(ReplaceCredentialBindingSchema, {
    connectionId: create(ProviderConnectionIdSchema, { value: input.connectionId }),
    previousBindingId: create(CredentialBindingIdSchema, { value: input.previousBindingId }),
    replacement: input.replacement,
    mode: input.mode,
    validation: input.validation,
  });
}

export async function buildReplacement(
  input: Omit<Parameters<typeof assembleReplacement>[0], 'replacement'> & { replacement: BindingDraft },
): Promise<ReplaceCredentialBinding> {
  return assembleReplacement({ ...input, replacement: await buildBinding(input.replacement) });
}
