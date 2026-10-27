import { AuthorizationScheme, OpenAiOutputLimitField, ProviderAccessScope, ProviderProtocol } from '@trogonai/proto';

import { emptyBindingValues } from './binding';
import { emptyModelRow, emptyRegistrationValues, type RegistrationFormValues } from './registration';

export interface Preset {
  readonly key: string;
  readonly label: string;
  readonly values: Partial<RegistrationFormValues>;
}

function managedModel(identifier: string) {
  return { ...emptyModelRow, identifier, pinModelId: identifier };
}

export const presets: readonly Preset[] = [
  {
    key: 'openai',
    label: 'OpenAI',
    values: {
      providerId: 'openai',
      endpointKind: 'network',
      baseUrl: 'https://api.openai.com/v1',
      accessScope: ProviderAccessScope.MANAGED,
      models: [managedModel('gpt-4.1')],
      protocols: [ProviderProtocol.OPENAI_CHAT_COMPLETIONS, ProviderProtocol.OPENAI_RESPONSES],
      outputLimit: OpenAiOutputLimitField.MAX_COMPLETION_TOKENS,
      binding: {
        ...emptyBindingValues,
        authenticationKind: 'stored-api-key',
        authorization: AuthorizationScheme.BEARER_TOKEN,
      },
    },
  },
  {
    key: 'anthropic',
    label: 'Anthropic',
    values: {
      providerId: 'anthropic',
      endpointKind: 'network',
      baseUrl: 'https://api.anthropic.com',
      accessScope: ProviderAccessScope.MANAGED,
      models: [managedModel('claude-sonnet-4-20250514')],
      protocols: [ProviderProtocol.ANTHROPIC_MESSAGES],
      outputLimit: OpenAiOutputLimitField.UNSPECIFIED,
      binding: {
        ...emptyBindingValues,
        authenticationKind: 'stored-api-key',
        authorization: AuthorizationScheme.X_API_KEY,
      },
    },
  },
  {
    key: 'gemini',
    label: 'Gemini',
    values: {
      providerId: 'google-gemini',
      endpointKind: 'network',
      baseUrl: 'https://generativelanguage.googleapis.com',
      accessScope: ProviderAccessScope.MANAGED,
      models: [managedModel('gemini-2.0-flash')],
      protocols: [ProviderProtocol.GEMINI_GENERATE_CONTENT],
      outputLimit: OpenAiOutputLimitField.UNSPECIFIED,
      binding: {
        ...emptyBindingValues,
        authenticationKind: 'stored-api-key',
        authorization: AuthorizationScheme.X_GOOG_API_KEY,
      },
    },
  },
  {
    key: 'bedrock',
    label: 'Bedrock',
    values: {
      providerId: 'aws-bedrock',
      endpointKind: 'network',
      baseUrl: 'https://bedrock-runtime.us-east-1.amazonaws.com',
      accessScope: ProviderAccessScope.MANAGED,
      models: [managedModel('anthropic.claude-sonnet-4-20250514-v1:0')],
      protocols: [ProviderProtocol.BEDROCK_CONVERSE, ProviderProtocol.BEDROCK_MODEL_INVOKE],
      outputLimit: OpenAiOutputLimitField.UNSPECIFIED,
      binding: {
        ...emptyBindingValues,
        authenticationKind: 'stored-api-key',
        authorization: AuthorizationScheme.BEARER_TOKEN,
      },
    },
  },
  {
    key: 'cloudflare-ai-gateway',
    label: 'Cloudflare AI Gateway',
    values: {
      providerId: 'cloudflare-ai-gateway',
      endpointKind: 'network',
      baseUrl: 'https://gateway.ai.cloudflare.com/v1/ACCOUNT/GATEWAY',
      accessScope: ProviderAccessScope.GATEWAY_ONLY,
      models: [{ ...emptyModelRow, identifier: 'openai/gpt-4o-mini' }],
      protocols: [ProviderProtocol.OPENAI_CHAT_COMPLETIONS],
      outputLimit: OpenAiOutputLimitField.INFER_FROM_MODEL,
      binding: {
        ...emptyBindingValues,
        authenticationKind: 'gateway',
        gatewayMode: 'configured-key',
        authorization: AuthorizationScheme.CLOUDFLARE_AI_GATEWAY,
      },
    },
  },
  {
    key: 'local',
    label: 'Local deployment',
    values: {
      providerId: 'ollama',
      endpointKind: 'deployment-local',
      endpointId: 'ollama-local',
      accessScope: ProviderAccessScope.MANAGED,
      models: [managedModel('llama3.1:8b')],
      protocols: [ProviderProtocol.OPENAI_CHAT_COMPLETIONS],
      outputLimit: OpenAiOutputLimitField.UNSPECIFIED,
      binding: { ...emptyBindingValues, authenticationKind: 'deployment-no-credential' },
    },
  },
];

export function applyPreset(current: RegistrationFormValues, preset: Preset): RegistrationFormValues {
  return {
    ...emptyRegistrationValues,
    ...preset.values,
    baseUrl: preset.values.baseUrl ?? '',
    endpointId: preset.values.endpointId ?? '',
    connectionId: current.connectionId,
    parentNodeId: current.parentNodeId,
    displayName: current.displayName,
    description: current.description,
    preference: current.preference,
    eligibility: current.eligibility,
    binding: {
      ...(preset.values.binding ?? emptyBindingValues),
      bindingId: current.binding.bindingId,
      secretRef: current.binding.secretRef,
    },
  };
}
