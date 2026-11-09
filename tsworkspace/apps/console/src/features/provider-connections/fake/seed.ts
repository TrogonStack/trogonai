import { create } from '@bufbuild/protobuf';
import {
  AuthorizationScheme,
  BindingReplacementMode,
  OpenAiOutputLimitField,
  ProviderAccessScope,
  ProviderProtocol,
  ProviderRoutingEligibility,
  ProviderValidationRefSchema,
} from '@trogonai/proto';

import { commands } from '../domain/commands';
import { digestHex, sha256Digest } from '../domain/digest';
import { buildRegistration, buildReplacement, type RegistrationDraft } from '../domain/draft';
import type { SeedWriter } from './in-memory-client';

const HOUR = 3_600_000;
const DAY = 24 * HOUR;
const PARENT_NODE = 'node_01h9xplatform';

async function catalogDigestHex(label: string): Promise<string> {
  return digestHex(await sha256Digest(new TextEncoder().encode(label)));
}

function validation(label: string) {
  return create(ProviderValidationRefSchema, { validationId: `validation-seed-${label}` });
}

export async function populateSeed(writer: SeedWriter, now: Date = new Date()): Promise<void> {
  const ago = (milliseconds: number) => new Date(now.getTime() - milliseconds);
  const dispatch = (command: Parameters<SeedWriter['dispatch']>[0], at: Date, expectedRevision?: number) =>
    writer.dispatch(command, { expectedRevision, commandId: `seed-${at.getTime()}-${command.case}` }, at);

  const openAi: RegistrationDraft = {
    connectionId: 'openai-prod',
    parentNodeId: PARENT_NODE,
    providerId: 'openai',
    endpoint: { kind: 'network', baseUrl: 'https://api.openai.com/v1' },
    accessScope: ProviderAccessScope.MANAGED,
    models: [
      {
        identifier: 'gpt-4.1',
        catalogPin: {
          modelId: 'gpt-4.1',
          version: '2025-04-14',
          definitionDigestHex: await catalogDigestHex('catalog/gpt-4.1@2025-04-14'),
        },
      },
      {
        identifier: 'gpt-4.1-mini',
        providerRevision: 'gpt-4.1-mini-2025-04-14',
        catalogPin: {
          modelId: 'gpt-4.1-mini',
          version: '2025-04-14',
          definitionDigestHex: await catalogDigestHex('catalog/gpt-4.1-mini@2025-04-14'),
        },
      },
    ],
    protocols: [ProviderProtocol.OPENAI_CHAT_COMPLETIONS, ProviderProtocol.OPENAI_RESPONSES],
    openAiOutputLimit: OpenAiOutputLimitField.MAX_COMPLETION_TOKENS,
    binding: {
      bindingId: 'openai-prod-key-1',
      authentication: {
        kind: 'stored-api-key',
        secretRef: 'secret://providers/openai-prod/key-1',
        authorization: AuthorizationScheme.BEARER_TOKEN,
      },
    },
    profile: { displayName: 'OpenAI production', description: 'Primary managed OpenAI account.' },
    routingPolicy: { preference: 10, eligibility: ProviderRoutingEligibility.ENABLED },
  };
  const t = (days: number, hours = 0) => ago(60 * DAY - days * DAY - hours * HOUR);
  await dispatch(commands.register(await buildRegistration(openAi)), t(0));
  await dispatch(commands.activate('openai-prod', validation('openai-1')), t(0, 1), 1);
  await dispatch(
    commands.replaceBinding(
      await buildReplacement({
        connectionId: 'openai-prod',
        previousBindingId: 'openai-prod-key-1',
        replacement: {
          bindingId: 'openai-prod-key-2',
          authentication: {
            kind: 'stored-api-key',
            secretRef: 'secret://providers/openai-prod/key-2',
            authorization: AuthorizationScheme.BEARER_TOKEN,
          },
        },
        mode: BindingReplacementMode.FAIL_IF_IN_USE,
        validation: validation('openai-2'),
      }),
    ),
    t(24),
    2,
  );
  await dispatch(
    commands.replaceBinding(
      await buildReplacement({
        connectionId: 'openai-prod',
        previousBindingId: 'openai-prod-key-2',
        replacement: {
          bindingId: 'openai-prod-key-3',
          authentication: {
            kind: 'stored-api-key',
            secretRef: 'secret://providers/openai-prod/key-3',
            authorization: AuthorizationScheme.BEARER_TOKEN,
          },
        },
        mode: BindingReplacementMode.DRAIN,
        validation: validation('openai-3'),
      }),
    ),
    t(52),
    3,
  );
  await dispatch(
    commands.setProfile('openai-prod', {
      displayName: 'OpenAI production',
      description: 'Primary managed OpenAI account. Keys rotate quarterly.',
    }),
    t(53),
    4,
  );
  await dispatch(
    commands.setRoutingPolicy('openai-prod', { preference: 20, eligibility: ProviderRoutingEligibility.ENABLED }),
    t(54),
    5,
  );
  writer.pinSessions('openai-prod', 'openai-prod-key-2', 2);
  writer.pinSessions('openai-prod', 'openai-prod-key-3', 3);

  await dispatch(
    commands.register(
      await buildRegistration({
        connectionId: 'anthropic-eval',
        parentNodeId: PARENT_NODE,
        providerId: 'anthropic',
        endpoint: { kind: 'network', baseUrl: 'https://api.anthropic.com' },
        accessScope: ProviderAccessScope.MANAGED,
        models: [
          {
            identifier: 'claude-sonnet-4-20250514',
            catalogPin: {
              modelId: 'claude-sonnet-4',
              version: '20250514',
              definitionDigestHex: await catalogDigestHex('catalog/claude-sonnet-4@20250514'),
            },
          },
        ],
        protocols: [ProviderProtocol.ANTHROPIC_MESSAGES],
        binding: {
          bindingId: 'anthropic-eval-key-1',
          authentication: {
            kind: 'stored-api-key',
            secretRef: 'secret://providers/anthropic-eval/key-1',
            authorization: AuthorizationScheme.X_API_KEY,
          },
        },
        profile: { displayName: 'Anthropic evaluation', description: 'Awaiting trusted validation.' },
        routingPolicy: { preference: 0, eligibility: ProviderRoutingEligibility.DISABLED },
      }),
    ),
    ago(2 * DAY),
  );

  await dispatch(
    commands.register(
      await buildRegistration({
        connectionId: 'cf-gateway-eu',
        parentNodeId: PARENT_NODE,
        providerId: 'cloudflare-ai-gateway',
        endpoint: { kind: 'network', baseUrl: 'https://gateway.ai.cloudflare.com/v1/example-account/trogon-eu' },
        accessScope: ProviderAccessScope.GATEWAY_ONLY,
        models: [{ identifier: 'openai/gpt-4o-mini' }, { identifier: 'workers-ai/@cf/meta/llama-3.1-8b-instruct' }],
        protocols: [ProviderProtocol.OPENAI_CHAT_COMPLETIONS],
        openAiOutputLimit: OpenAiOutputLimitField.INFER_FROM_MODEL,
        binding: {
          bindingId: 'cf-gateway-eu-token-1',
          authentication: {
            kind: 'gateway',
            authorization: AuthorizationScheme.CLOUDFLARE_AI_GATEWAY,
            mode: { kind: 'configured-key', secretRef: 'secret://providers/cf-gateway-eu/token-1' },
          },
        },
        profile: { displayName: 'Cloudflare AI Gateway (EU)', description: 'Gateway-only access, not managed.' },
        routingPolicy: { preference: 0, eligibility: ProviderRoutingEligibility.ENABLED },
      }),
    ),
    ago(14 * DAY),
  );
  await dispatch(commands.activate('cf-gateway-eu', validation('cf-1')), ago(14 * DAY - HOUR), 1);

  await dispatch(
    commands.register(
      await buildRegistration({
        connectionId: 'gemini-legacy',
        parentNodeId: PARENT_NODE,
        providerId: 'google-gemini',
        endpoint: { kind: 'network', baseUrl: 'https://generativelanguage.googleapis.com' },
        accessScope: ProviderAccessScope.MANAGED,
        models: [
          {
            identifier: 'gemini-2.0-flash',
            catalogPin: {
              modelId: 'gemini-2.0-flash',
              version: '001',
              definitionDigestHex: await catalogDigestHex('catalog/gemini-2.0-flash@001'),
            },
          },
        ],
        protocols: [ProviderProtocol.GEMINI_GENERATE_CONTENT],
        binding: {
          bindingId: 'gemini-legacy-key-1',
          authentication: {
            kind: 'stored-api-key',
            secretRef: 'secret://providers/gemini-legacy/key-1',
            authorization: AuthorizationScheme.X_GOOG_API_KEY,
          },
        },
        profile: { displayName: 'Gemini (legacy project)', description: 'Retired after the project migration.' },
        routingPolicy: { preference: -10, eligibility: ProviderRoutingEligibility.DISABLED },
      }),
    ),
    ago(45 * DAY),
  );
  await dispatch(commands.activate('gemini-legacy', validation('gemini-1')), ago(45 * DAY - HOUR), 1);
  await dispatch(commands.revokeConnection('gemini-legacy'), ago(9 * DAY), 2);

  await dispatch(
    commands.register(
      await buildRegistration({
        connectionId: 'ollama-local',
        parentNodeId: PARENT_NODE,
        providerId: 'ollama',
        endpoint: { kind: 'deployment-local', endpointId: 'ollama-local' },
        accessScope: ProviderAccessScope.MANAGED,
        models: [
          {
            identifier: 'llama3.1:8b',
            catalogPin: {
              modelId: 'llama-3.1-8b-instruct',
              version: '1',
              definitionDigestHex: await catalogDigestHex('catalog/llama-3.1-8b-instruct@1'),
            },
          },
        ],
        protocols: [ProviderProtocol.OPENAI_CHAT_COMPLETIONS],
        binding: { bindingId: 'ollama-local-default', authentication: { kind: 'deployment-no-credential' } },
        profile: { displayName: 'Local Ollama', description: 'Deployment-local auth-free endpoint.' },
        routingPolicy: { preference: -5, eligibility: ProviderRoutingEligibility.ENABLED },
      }),
    ),
    ago(6 * DAY),
  );
  await dispatch(commands.activate('ollama-local', validation('ollama-1')), ago(6 * DAY - HOUR), 1);
  writer.pinSessions('ollama-local', 'ollama-local-default', 1);
}
