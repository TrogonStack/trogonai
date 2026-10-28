import {
  AuthorizationScheme,
  BindingReplacementMode,
  CredentialBindingLifecycle,
  OpenAiOutputLimitField,
  ProviderAccessScope,
  ProviderConnectionLifecycle,
  ProviderProtocol,
  ProviderRoutingEligibility,
} from '@trogonai/proto';

export const protocolLabels: Record<ProviderProtocol, string> = {
  [ProviderProtocol.UNSPECIFIED]: 'Unspecified',
  [ProviderProtocol.OPENAI_CHAT_COMPLETIONS]: 'OpenAI Chat Completions',
  [ProviderProtocol.OPENAI_RESPONSES]: 'OpenAI Responses',
  [ProviderProtocol.ANTHROPIC_MESSAGES]: 'Anthropic Messages',
  [ProviderProtocol.GEMINI_GENERATE_CONTENT]: 'Gemini generateContent',
  [ProviderProtocol.BEDROCK_MODEL_INVOKE]: 'Bedrock InvokeModel',
  [ProviderProtocol.BEDROCK_CONVERSE]: 'Bedrock Converse',
  [ProviderProtocol.GOOGLE_GENERATE_CONTENT]: 'Google generateContent',
  [ProviderProtocol.GOOGLE_RAW_PREDICT]: 'Google rawPredict',
};

export const selectableProtocols: readonly ProviderProtocol[] = [
  ProviderProtocol.OPENAI_CHAT_COMPLETIONS,
  ProviderProtocol.OPENAI_RESPONSES,
  ProviderProtocol.ANTHROPIC_MESSAGES,
  ProviderProtocol.GEMINI_GENERATE_CONTENT,
  ProviderProtocol.BEDROCK_MODEL_INVOKE,
  ProviderProtocol.BEDROCK_CONVERSE,
  ProviderProtocol.GOOGLE_GENERATE_CONTENT,
  ProviderProtocol.GOOGLE_RAW_PREDICT,
];

export const authorizationLabels: Record<AuthorizationScheme, string> = {
  [AuthorizationScheme.UNSPECIFIED]: 'None',
  [AuthorizationScheme.BEARER_TOKEN]: 'Bearer token',
  [AuthorizationScheme.X_API_KEY]: 'x-api-key',
  [AuthorizationScheme.X_GOOG_API_KEY]: 'x-goog-api-key',
  [AuthorizationScheme.HEC_TOKEN]: 'HEC token',
  [AuthorizationScheme.CLOUDFLARE_AI_GATEWAY]: 'Cloudflare AI Gateway',
};

export const selectableAuthorizationSchemes: readonly AuthorizationScheme[] = [
  AuthorizationScheme.BEARER_TOKEN,
  AuthorizationScheme.X_API_KEY,
  AuthorizationScheme.X_GOOG_API_KEY,
  AuthorizationScheme.HEC_TOKEN,
  AuthorizationScheme.CLOUDFLARE_AI_GATEWAY,
];

export const accessScopeLabels: Record<ProviderAccessScope, string> = {
  [ProviderAccessScope.UNSPECIFIED]: 'Unspecified',
  [ProviderAccessScope.MANAGED]: 'Managed',
  [ProviderAccessScope.GATEWAY_ONLY]: 'Gateway only',
};

export const outputLimitLabels: Record<OpenAiOutputLimitField, string> = {
  [OpenAiOutputLimitField.UNSPECIFIED]: 'Not set',
  [OpenAiOutputLimitField.INFER_FROM_MODEL]: 'Infer from model',
  [OpenAiOutputLimitField.MAX_TOKENS]: 'max_tokens',
  [OpenAiOutputLimitField.MAX_COMPLETION_TOKENS]: 'max_completion_tokens',
};

export const selectableOutputLimits: readonly OpenAiOutputLimitField[] = [
  OpenAiOutputLimitField.INFER_FROM_MODEL,
  OpenAiOutputLimitField.MAX_TOKENS,
  OpenAiOutputLimitField.MAX_COMPLETION_TOKENS,
];

export const connectionLifecycleLabels: Record<ProviderConnectionLifecycle, string> = {
  [ProviderConnectionLifecycle.UNSPECIFIED]: 'Unspecified',
  [ProviderConnectionLifecycle.PENDING]: 'Pending',
  [ProviderConnectionLifecycle.ACTIVE]: 'Active',
  [ProviderConnectionLifecycle.REVOKED]: 'Revoked',
};

export const bindingLifecycleLabels: Record<CredentialBindingLifecycle, string> = {
  [CredentialBindingLifecycle.UNSPECIFIED]: 'Unspecified',
  [CredentialBindingLifecycle.PENDING]: 'Pending',
  [CredentialBindingLifecycle.ACTIVE]: 'Active',
  [CredentialBindingLifecycle.DRAINING]: 'Draining',
  [CredentialBindingLifecycle.REVOKED]: 'Revoked',
};

export const eligibilityLabels: Record<ProviderRoutingEligibility, string> = {
  [ProviderRoutingEligibility.UNSPECIFIED]: 'Unspecified',
  [ProviderRoutingEligibility.ENABLED]: 'Enabled',
  [ProviderRoutingEligibility.DISABLED]: 'Disabled',
};

export const replacementModeLabels: Record<BindingReplacementMode, string> = {
  [BindingReplacementMode.UNSPECIFIED]: 'Unspecified',
  [BindingReplacementMode.DRAIN]: 'Drain: previous binding serves pinned Sessions until they end',
  [BindingReplacementMode.FAIL_IF_IN_USE]: 'Fail if in use: revoke the previous binding, reject if any Session pins it',
};
