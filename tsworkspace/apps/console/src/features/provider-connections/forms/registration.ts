import {
  AuthorizationScheme,
  OpenAiOutputLimitField,
  ProviderAccessScope,
  ProviderProtocol,
  ProviderRoutingEligibility,
} from '@trogonai/proto';
import { z } from 'zod';

import type { ModelDraft, RegistrationDraft } from '../domain/draft';
import { isOpenAiProtocol, isValidBaseUrl } from '../domain/validation';
import { bindingFieldsSchema, checkBindingValues, emptyBindingValues, toBindingDraft } from './binding';

const DIGEST_HEX = /^[0-9a-fA-F]{64}$/;
const SINT32_MAX = 2 ** 31 - 1;

export const modelRowSchema = z.object({
  identifier: z.string(),
  providerRevision: z.string(),
  pinModelId: z.string(),
  pinVersion: z.string(),
  pinDigestHex: z.string(),
});

export type ModelRowValues = z.infer<typeof modelRowSchema>;

export const emptyModelRow: ModelRowValues = {
  identifier: '',
  providerRevision: '',
  pinModelId: '',
  pinVersion: '',
  pinDigestHex: '',
};

export const registrationFormSchema = z
  .object({
    connectionId: z.string(),
    parentNodeId: z.string(),
    providerId: z.string(),
    endpointKind: z.enum(['network', 'deployment-local']),
    baseUrl: z.string(),
    endpointId: z.string(),
    accessScope: z.number(),
    models: z.array(modelRowSchema),
    protocols: z.array(z.number()),
    outputLimit: z.number(),
    binding: bindingFieldsSchema,
    displayName: z.string(),
    description: z.string(),
    preference: z.number({ error: 'Preference must be a whole number.' }),
    eligibility: z.number(),
  })
  .superRefine((values, context) => {
    const add = (path: (string | number)[], message: string) => context.addIssue({ code: 'custom', path, message });

    if (!values.connectionId.trim()) add(['connectionId'], 'Connection id is required.');
    if (!values.parentNodeId.trim()) add(['parentNodeId'], 'Parent node id is required.');
    if (!values.providerId.trim()) add(['providerId'], 'Provider id is required.');

    if (values.endpointKind === 'network') {
      if (!isValidBaseUrl(values.baseUrl.trim())) {
        add(
          ['baseUrl'],
          'Use a credential-free https URL without query or fragment, for example https://api.example.com/v1.',
        );
      }
    } else if (!values.endpointId.trim()) {
      add(['endpointId'], 'Deployment endpoint id is required.');
    }

    if (values.accessScope !== ProviderAccessScope.MANAGED && values.accessScope !== ProviderAccessScope.GATEWAY_ONLY) {
      add(['accessScope'], 'Select an access scope.');
    }

    if (values.models.length === 0) add(['models'], 'At least one allowed model is required.');
    const seen = new Set<string>();
    values.models.forEach((model, index) => {
      const identifier = model.identifier.trim();
      if (!identifier) {
        add(['models', index, 'identifier'], 'Model identifier is required.');
      } else if (seen.has(identifier)) {
        add(['models', index, 'identifier'], 'Provider model identifiers must be unique.');
      }
      seen.add(identifier);

      const pinStarted = Boolean(model.pinModelId.trim() || model.pinVersion.trim() || model.pinDigestHex.trim());
      if (pinStarted || values.accessScope === ProviderAccessScope.MANAGED) {
        const managedHint =
          values.accessScope === ProviderAccessScope.MANAGED
            ? 'Managed offerings require an exact catalog pin.'
            : 'Complete the catalog pin.';
        if (!model.pinModelId.trim())
          add(['models', index, 'pinModelId'], `Catalog model id is required. ${managedHint}`);
        if (!model.pinVersion.trim()) add(['models', index, 'pinVersion'], 'Catalog version is required.');
        if (!DIGEST_HEX.test(model.pinDigestHex.trim())) {
          add(['models', index, 'pinDigestHex'], 'Definition digest must be 64 hex characters (sha256).');
        }
      }
    });

    if (values.protocols.length === 0) add(['protocols'], 'Select at least one protocol.');

    if (!Number.isInteger(values.preference) || Math.abs(values.preference) > SINT32_MAX) {
      add(['preference'], 'Preference must be a whole number.');
    }
    if (
      values.eligibility !== ProviderRoutingEligibility.ENABLED &&
      values.eligibility !== ProviderRoutingEligibility.DISABLED
    ) {
      add(['eligibility'], 'Select routing eligibility.');
    }

    checkBindingValues(values.binding, { accessScope: values.accessScope, endpointKind: values.endpointKind }, add, [
      'binding',
    ]);
  });

export function isOpenAiProtocolValue(value: number): boolean {
  return isOpenAiProtocol(value as ProviderProtocol);
}

export type RegistrationFormValues = z.infer<typeof registrationFormSchema>;

export const emptyRegistrationValues: RegistrationFormValues = {
  connectionId: '',
  parentNodeId: '',
  providerId: '',
  endpointKind: 'network',
  baseUrl: '',
  endpointId: '',
  accessScope: ProviderAccessScope.MANAGED,
  models: [{ ...emptyModelRow }],
  protocols: [],
  outputLimit: OpenAiOutputLimitField.UNSPECIFIED,
  binding: { ...emptyBindingValues, authorization: AuthorizationScheme.BEARER_TOKEN },
  displayName: '',
  description: '',
  preference: 0,
  eligibility: ProviderRoutingEligibility.ENABLED,
};

function toModelDraft(row: ModelRowValues): ModelDraft {
  const pinStarted = Boolean(row.pinModelId.trim() || row.pinVersion.trim() || row.pinDigestHex.trim());
  return {
    identifier: row.identifier.trim(),
    providerRevision: row.providerRevision.trim() || undefined,
    catalogPin: pinStarted
      ? {
          modelId: row.pinModelId.trim(),
          version: row.pinVersion.trim(),
          definitionDigestHex: row.pinDigestHex.trim().toLowerCase(),
        }
      : undefined,
  };
}

export function toRegistrationDraft(values: RegistrationFormValues): RegistrationDraft {
  return {
    connectionId: values.connectionId.trim(),
    parentNodeId: values.parentNodeId.trim(),
    providerId: values.providerId.trim(),
    endpoint:
      values.endpointKind === 'network'
        ? { kind: 'network', baseUrl: values.baseUrl.trim() }
        : { kind: 'deployment-local', endpointId: values.endpointId.trim() },
    accessScope: values.accessScope as ProviderAccessScope,
    models: values.models.map(toModelDraft),
    protocols: values.protocols as ProviderProtocol[],
    openAiOutputLimit:
      values.outputLimit === OpenAiOutputLimitField.UNSPECIFIED || !values.protocols.some(isOpenAiProtocolValue)
        ? undefined
        : (values.outputLimit as OpenAiOutputLimitField),
    binding: toBindingDraft(values.binding),
    profile: { displayName: values.displayName.trim(), description: values.description.trim() },
    routingPolicy: { preference: values.preference, eligibility: values.eligibility as ProviderRoutingEligibility },
  };
}
