import {
  AuthorizationScheme,
  BindingReplacementMode,
  OpenAiOutputLimitField,
  ProviderAccessScope,
  ProviderConnectionLifecycle,
  ProviderProtocol,
  ProviderRoutingEligibility,
  type CredentialBinding,
  type CredentialBindingConfiguration,
  type Digest,
  type ProviderConnectionConfiguration,
  type ProviderConnectionRegistration,
  type ProviderRoutingPolicy,
} from '@trogonai/proto';

import type { Violation } from './faults';

export const CONTRACT_VERSION = '1';
export const BASE_URL_PATTERN = /^https:\/\/[^/?#@]+(\/[^?#]*)?$/;
const SINT32_MIN = -(2 ** 31);
const SINT32_MAX = 2 ** 31 - 1;

const OPENAI_PROTOCOLS: readonly ProviderProtocol[] = [
  ProviderProtocol.OPENAI_CHAT_COMPLETIONS,
  ProviderProtocol.OPENAI_RESPONSES,
];

export function isOpenAiProtocol(protocol: ProviderProtocol): boolean {
  return OPENAI_PROTOCOLS.includes(protocol);
}

function violation(path: string, constraint: string, message: string): Violation {
  return { path, constraint, message };
}

function knownNonZero<T extends number>(enumeration: Record<number, string>, value: T): boolean {
  return value !== 0 && enumeration[value] !== undefined;
}

export function isValidBaseUrl(value: string): boolean {
  if (!BASE_URL_PATTERN.test(value)) return false;
  try {
    new URL(value);
    return true;
  } catch {
    return false;
  }
}

function validateDigest(path: string, digest: Digest | undefined): Violation[] {
  if (!digest) return [violation(path, 'required', 'digest is required')];
  const violations: Violation[] = [];
  if (digest.algorithm !== 'sha256')
    violations.push(violation(`${path}.algorithm`, 'const', 'algorithm must be sha256'));
  if (digest.value.length !== 32) violations.push(violation(`${path}.value`, 'len', 'digest value must be 32 bytes'));
  return violations;
}

export function validateConfiguration(configuration: ProviderConnectionConfiguration): Violation[] {
  const violations: Violation[] = [];
  const at = 'configuration';

  if (!configuration.parent?.value) violations.push(violation(`${at}.parent`, 'min_len', 'parent node id is required'));
  if (!configuration.provider?.value)
    violations.push(violation(`${at}.provider`, 'min_len', 'provider id is required'));

  const endpoint = configuration.endpoint?.endpoint;
  if (!endpoint || endpoint.case === undefined) {
    violations.push(violation(`${at}.endpoint`, 'oneof.required', 'exactly one endpoint is required'));
  } else if (endpoint.case === 'network') {
    if (!isValidBaseUrl(endpoint.value.baseUrl)) {
      violations.push(
        violation(
          `${at}.endpoint.network.base_url`,
          'string.pattern',
          'base URL must be a credential-free https URL without query or fragment',
        ),
      );
    }
  } else if (!endpoint.value.endpointId) {
    violations.push(violation(`${at}.endpoint.deployment_local.endpoint_id`, 'min_len', 'endpoint id is required'));
  }

  if (!knownNonZero(ProviderAccessScope, configuration.accessScope)) {
    violations.push(violation(`${at}.access_scope`, 'enum.defined_only', 'access scope must be a known value'));
  }

  if (configuration.allowedModels.length === 0) {
    violations.push(violation(`${at}.allowed_models`, 'repeated.min_items', 'at least one model is required'));
  }
  const seen = new Set<string>();
  configuration.allowedModels.forEach((model, index) => {
    const path = `${at}.allowed_models[${index}]`;
    const identifier = model.identifier?.value ?? '';
    if (!identifier) {
      violations.push(violation(`${path}.identifier`, 'min_len', 'model identifier is required'));
    } else if (seen.has(identifier)) {
      violations.push(
        violation(`${path}.identifier`, 'connection.models_unique', 'provider model identifiers must be unique'),
      );
    }
    seen.add(identifier);
    if (model.catalogPin) {
      const ref = model.catalogPin.versionRef;
      if (!ref?.modelId)
        violations.push(
          violation(`${path}.catalog_pin.version_ref.model_id`, 'min_len', 'catalog model id is required'),
        );
      if (!ref?.version)
        violations.push(violation(`${path}.catalog_pin.version_ref.version`, 'min_len', 'catalog version is required'));
      violations.push(...validateDigest(`${path}.catalog_pin.definition_digest`, model.catalogPin.definitionDigest));
    } else if (configuration.accessScope === ProviderAccessScope.MANAGED) {
      violations.push(
        violation(
          `${path}.catalog_pin`,
          'connection.managed_catalog_pins',
          'managed offerings require exact catalog pins',
        ),
      );
    }
  });

  if (configuration.supportedProtocols.length === 0) {
    violations.push(violation(`${at}.supported_protocols`, 'repeated.min_items', 'at least one protocol is required'));
  }
  if (new Set(configuration.supportedProtocols).size !== configuration.supportedProtocols.length) {
    violations.push(violation(`${at}.supported_protocols`, 'repeated.unique', 'protocols must be unique'));
  }
  if (configuration.supportedProtocols.some((protocol) => !knownNonZero(ProviderProtocol, protocol))) {
    violations.push(violation(`${at}.supported_protocols`, 'enum.defined_only', 'protocols must be known values'));
  }

  const limit = configuration.openAiOutputLimitField;
  if (limit !== OpenAiOutputLimitField.UNSPECIFIED) {
    if (!knownNonZero(OpenAiOutputLimitField, limit)) {
      violations.push(
        violation(`${at}.open_ai_output_limit_field`, 'enum.defined_only', 'output limit field must be a known value'),
      );
    }
    if (!configuration.supportedProtocols.some(isOpenAiProtocol)) {
      violations.push(
        violation(
          `${at}.open_ai_output_limit_field`,
          'connection.output_limit_protocol',
          'output-limit selection requires an OpenAI protocol',
        ),
      );
    }
  }

  if (configuration.contractVersion !== CONTRACT_VERSION) {
    violations.push(violation(`${at}.contract_version`, 'string.const', 'only contract version 1 is supported'));
  }
  return violations;
}

export function validateBindingConfiguration(configuration: CredentialBindingConfiguration, at: string): Violation[] {
  const violations: Violation[] = [];
  if (configuration.contractVersion !== CONTRACT_VERSION) {
    violations.push(violation(`${at}.contract_version`, 'string.const', 'only contract version 1 is supported'));
  }
  const authentication = configuration.authentication;
  switch (authentication.case) {
    case undefined:
      violations.push(
        violation(`${at}.authentication`, 'oneof.required', 'exactly one authentication method is required'),
      );
      break;
    case 'storedApiKey':
      if (!authentication.value.secretRef?.value) {
        violations.push(violation(`${at}.stored_api_key.secret_ref`, 'min_len', 'secret reference is required'));
      }
      if (!knownNonZero(AuthorizationScheme, authentication.value.authorization)) {
        violations.push(
          violation(`${at}.stored_api_key.authorization`, 'enum.defined_only', 'authorization scheme is required'),
        );
      }
      break;
    case 'deploymentNoCredential':
      break;
    case 'gateway': {
      const gateway = authentication.value;
      const hasAuthorization = gateway.authorization !== AuthorizationScheme.UNSPECIFIED;
      if (hasAuthorization && !knownNonZero(AuthorizationScheme, gateway.authorization)) {
        violations.push(
          violation(`${at}.gateway.authorization`, 'enum.defined_only', 'authorization must be a known value'),
        );
      }
      switch (gateway.mode.case) {
        case undefined:
          violations.push(violation(`${at}.gateway.mode`, 'oneof.required', 'exactly one gateway mode is required'));
          break;
        case 'configuredKey':
        case 'overrideKey':
          if (!hasAuthorization) {
            violations.push(
              violation(
                `${at}.gateway.authorization`,
                'gateway_auth.key_scheme_required',
                'configured and override key modes require authorization',
              ),
            );
          }
          if (gateway.mode.case === 'overrideKey' && !gateway.mode.value.secretRef?.value) {
            violations.push(
              violation(`${at}.gateway.override_key.secret_ref`, 'min_len', 'secret reference is required'),
            );
          }
          if (
            gateway.mode.case === 'configuredKey' &&
            gateway.mode.value.secretRef &&
            !gateway.mode.value.secretRef.value
          ) {
            violations.push(
              violation(`${at}.gateway.configured_key.secret_ref`, 'min_len', 'secret reference must not be empty'),
            );
          }
          break;
        default:
          break;
      }
      break;
    }
  }
  return violations;
}

export function validateBinding(binding: CredentialBinding, at: string): Violation[] {
  const violations: Violation[] = [];
  if (!binding.id?.value) violations.push(violation(`${at}.id`, 'min_len', 'binding id is required'));
  if (!binding.configuration) {
    violations.push(violation(`${at}.configuration`, 'required', 'binding configuration is required'));
  } else {
    violations.push(...validateBindingConfiguration(binding.configuration, `${at}.configuration`));
  }
  violations.push(...validateDigest(`${at}.configuration_digest`, binding.configurationDigest));
  if (binding.configurationBytes.length === 0) {
    violations.push(violation(`${at}.configuration_bytes`, 'bytes.min_len', 'configuration bytes are required'));
  }
  return violations;
}

export function validateRegistration(registration: ProviderConnectionRegistration): Violation[] {
  const violations: Violation[] = [];
  if (!registration.id?.value) violations.push(violation('id', 'min_len', 'connection id is required'));
  if (!registration.configuration) {
    violations.push(violation('configuration', 'required', 'configuration is required'));
  } else {
    violations.push(...validateConfiguration(registration.configuration));
  }
  violations.push(...validateDigest('configuration_digest', registration.configurationDigest));
  if (registration.configurationBytes.length === 0) {
    violations.push(violation('configuration_bytes', 'bytes.min_len', 'configuration bytes are required'));
  }
  if (!registration.initialBinding) {
    violations.push(violation('initial_binding', 'required', 'initial binding is required'));
  } else {
    violations.push(...validateBinding(registration.initialBinding, 'initial_binding'));
  }
  if (!registration.profile) violations.push(violation('profile', 'required', 'profile is required'));
  if (!registration.routingPolicy) {
    violations.push(violation('routing_policy', 'required', 'routing policy is required'));
  } else {
    violations.push(...validateRoutingPolicy(registration.routingPolicy, 'routing_policy'));
  }
  return violations;
}

export function validateRoutingPolicy(policy: ProviderRoutingPolicy, at: string): Violation[] {
  const violations: Violation[] = [];
  if (!Number.isInteger(policy.preference) || policy.preference < SINT32_MIN || policy.preference > SINT32_MAX) {
    violations.push(violation(`${at}.preference`, 'sint32', 'preference must be a 32-bit integer'));
  }
  if (!knownNonZero(ProviderRoutingEligibility, policy.eligibility)) {
    violations.push(violation(`${at}.eligibility`, 'enum.defined_only', 'eligibility must be a known value'));
  }
  return violations;
}

export function validateReplacementMode(mode: BindingReplacementMode): Violation[] {
  return knownNonZero(BindingReplacementMode, mode)
    ? []
    : [violation('mode', 'enum.defined_only', 'replacement mode must be a known value')];
}

export function bindingScopeMismatch(
  configuration: ProviderConnectionConfiguration,
  binding: CredentialBindingConfiguration,
): string | undefined {
  const authentication = binding.authentication.case;
  if (configuration.accessScope === ProviderAccessScope.MANAGED) {
    if (authentication === 'storedApiKey') return undefined;
    if (authentication === 'deploymentNoCredential') {
      return configuration.endpoint?.endpoint.case === 'deploymentLocal'
        ? undefined
        : 'a binding without credentials requires a deployment-local endpoint';
    }
    return 'managed scope accepts stored API keys, or no credential on a deployment-local endpoint';
  }
  return authentication === 'gateway' ? undefined : 'gateway-only scope requires gateway authentication';
}

export function isLive(lifecycle: ProviderConnectionLifecycle): boolean {
  return lifecycle === ProviderConnectionLifecycle.PENDING || lifecycle === ProviderConnectionLifecycle.ACTIVE;
}
