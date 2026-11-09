import { create, equals, fromBinary } from '@bufbuild/protobuf';
import {
  CredentialBindingConfigurationSchema,
  CredentialBindingLifecycle,
  ProviderConnectionConfigurationSchema,
  ProviderValidationRefSchema,
  type CredentialBinding,
  type ProviderConnection,
  type ProviderValidationRef,
  type State,
} from '@trogonai/proto';

import type {
  DispatchOptions,
  ProviderConnectionRecord,
  ProviderConnectionsClient,
  StoredEvent,
  ValidationRequest,
} from '../client';
import {
  commandConnectionId,
  decide,
  fold,
  initialState,
  presentConnection,
  type DecisionContext,
  type ProviderCommand,
} from '../domain/decider';
import { digestsEqual, sha256Digest } from '../domain/digest';
import { ConcurrencyFault, ProviderConnectionFault, invalid } from '../domain/faults';

export const MAX_DRAINING_BINDINGS = 2;
const ACTOR = 'console-operator';

export interface SeedWriter {
  dispatch(command: ProviderCommand, options: DispatchOptions, at: Date): Promise<ProviderConnectionRecord>;
  pinSessions(connectionId: string, bindingId: string, count: number): void;
}

export interface InMemoryClientOptions {
  readonly latencyMs?: number;
  readonly clock?: () => Date;
}

function sleep(milliseconds: number): Promise<void> {
  return milliseconds > 0 ? new Promise((resolve) => setTimeout(resolve, milliseconds)) : Promise.resolve();
}

function secretRefsOf(binding: CredentialBinding): string[] {
  const authentication = binding.configuration?.authentication;
  if (authentication?.case === 'storedApiKey') return [authentication.value.secretRef?.value ?? ''];
  if (authentication?.case === 'gateway') {
    const mode = authentication.value.mode;
    if (mode.case === 'configuredKey' || mode.case === 'overrideKey') return [mode.value.secretRef?.value ?? ''];
  }
  return [];
}

export class InMemoryProviderConnectionsClient implements ProviderConnectionsClient {
  private readonly streams = new Map<string, StoredEvent[]>();
  private readonly pins = new Map<string, Map<string, number>>();
  private latencyMs: number;
  private clock: () => Date;
  private seed: () => Promise<void> = () => Promise.resolve();
  private ready: Promise<void> | undefined;

  constructor(options: InMemoryClientOptions = {}) {
    this.latencyMs = options.latencyMs ?? 0;
    this.clock = options.clock ?? (() => new Date());
  }

  seedWith(populate: (writer: SeedWriter) => Promise<void>): void {
    this.seed = () =>
      populate({
        dispatch: (command, options, at) => this.execute(command, options, at),
        pinSessions: (connectionId, bindingId, count) => {
          const pins = this.pins.get(connectionId) ?? new Map<string, number>();
          pins.set(bindingId, count);
          this.pins.set(connectionId, pins);
        },
      });
  }

  private async pause(): Promise<void> {
    this.ready ??= this.seed();
    await this.ready;
    await sleep(this.latencyMs);
  }

  async list(): Promise<readonly ProviderConnectionRecord[]> {
    await this.pause();
    return [...this.streams.keys()]
      .map((id) => this.record(id))
      .filter((record): record is ProviderConnectionRecord => record !== undefined);
  }

  async get(connectionId: string): Promise<ProviderConnectionRecord | undefined> {
    await this.pause();
    return this.record(connectionId);
  }

  async history(connectionId: string): Promise<readonly StoredEvent[]> {
    await this.pause();
    return [...(this.streams.get(connectionId) ?? [])];
  }

  async requestValidation(request: ValidationRequest): Promise<ProviderValidationRef> {
    await this.pause();
    if (!this.streams.has(request.connectionId)) throw new ProviderConnectionFault('PROVIDER_CONNECTION_NOT_FOUND');
    if (secretRefsOf(request.binding).some((ref) => ref.toLowerCase().includes('reject'))) {
      throw new ProviderConnectionFault(
        'PROVIDER_VALIDATION_REJECTED',
        'The simulated provider refused the credential (secret reference contains "reject").',
      );
    }
    return create(ProviderValidationRefSchema, { validationId: `validation-${crypto.randomUUID().slice(0, 8)}` });
  }

  async dispatch(command: ProviderCommand, options: DispatchOptions = {}): Promise<ProviderConnectionRecord> {
    await this.pause();
    return this.execute(command, options);
  }

  private async execute(
    command: ProviderCommand,
    options: DispatchOptions = {},
    at?: Date,
  ): Promise<ProviderConnectionRecord> {
    const connectionId = commandConnectionId(command);
    if (!connectionId) {
      throw invalid([{ path: 'connection_id', constraint: 'min_len', message: 'connection id is required' }]);
    }
    const stream = this.streams.get(connectionId) ?? [];
    const commandId = options.commandId ?? crypto.randomUUID();

    if (stream.some((stored) => stored.commandId === commandId)) return this.record(connectionId)!;

    if (command.case !== 'register' && stream.length > 0) {
      if (options.expectedRevision === undefined || options.expectedRevision < 1) {
        throw new ConcurrencyFault(
          'EXPECTED_REVISION_REQUIRED',
          'Commands on an existing stream require a positive expected revision.',
        );
      }
      if (options.expectedRevision !== stream.length) {
        throw new ConcurrencyFault(
          'REVISION_MISMATCH',
          `The connection is at revision ${stream.length}, not ${options.expectedRevision}. Reload and try again.`,
        );
      }
    }

    const state = this.stateOf(stream);
    const decision = decide(state, command, this.contextFor(connectionId));
    if (!decision.ok) throw decision.fault;

    await this.admit(command);

    const occurredAt = at ?? this.clock();
    const appended = decision.events.map<StoredEvent>((event, index) => ({
      revision: stream.length + index + 1,
      event,
      occurredAt,
      actor: ACTOR,
      commandId,
    }));
    const next = [...stream, ...appended];
    this.streams.set(connectionId, next);
    this.releaseRevokedPins(connectionId, this.stateOf(next));
    return this.record(connectionId)!;
  }

  async endPinnedSessions(connectionId: string, bindingId: string): Promise<void> {
    await this.pause();
    this.pins.get(connectionId)?.set(bindingId, 0);
  }

  private async admit(command: ProviderCommand): Promise<void> {
    if (command.case === 'register') {
      const registration = command.value.registration!;
      const decoded = fromBinary(ProviderConnectionConfigurationSchema, registration.configurationBytes);
      const digest = await sha256Digest(registration.configurationBytes);
      if (
        !digestsEqual(digest, registration.configurationDigest!) ||
        !equals(ProviderConnectionConfigurationSchema, decoded, registration.configuration!)
      ) {
        throw invalid([
          {
            path: 'registration.configuration_digest',
            constraint: 'digest',
            message: 'configuration digest or bytes do not match the typed configuration',
          },
        ]);
      }
      await this.admitBinding(registration.initialBinding!, 'registration.initial_binding');
    }
    if (command.case === 'replaceBinding') {
      await this.admitBinding(command.value.replacement!, 'replacement');
    }
  }

  private async admitBinding(binding: CredentialBinding, path: string): Promise<void> {
    const decoded = fromBinary(CredentialBindingConfigurationSchema, binding.configurationBytes);
    const digest = await sha256Digest(binding.configurationBytes);
    if (
      !digestsEqual(digest, binding.configurationDigest!) ||
      !equals(CredentialBindingConfigurationSchema, decoded, binding.configuration!)
    ) {
      throw invalid([
        {
          path: `${path}.configuration_digest`,
          constraint: 'digest',
          message: 'binding digest or bytes do not match the typed configuration',
        },
      ]);
    }
  }

  private stateOf(stream: readonly StoredEvent[]): State {
    return stream.length === 0 ? initialState() : fold(stream.map((stored) => stored.event));
  }

  private contextFor(connectionId: string): DecisionContext {
    return {
      pinnedSessions: (bindingId) => this.pins.get(connectionId)?.get(bindingId) ?? 0,
      permitsDrainOverlap: (connection: ProviderConnection) =>
        connection.bindings.filter((entry) => entry.lifecycle === CredentialBindingLifecycle.DRAINING).length <
        MAX_DRAINING_BINDINGS,
    };
  }

  private releaseRevokedPins(connectionId: string, state: State): void {
    const connection = presentConnection(state);
    const pins = this.pins.get(connectionId);
    if (!connection || !pins) return;
    for (const entry of connection.bindings) {
      if (entry.lifecycle === CredentialBindingLifecycle.REVOKED) pins.set(entry.binding!.id!.value, 0);
    }
  }

  private record(connectionId: string): ProviderConnectionRecord | undefined {
    const stream = this.streams.get(connectionId);
    if (!stream || stream.length === 0) return undefined;
    const connection = presentConnection(this.stateOf(stream));
    if (!connection) return undefined;
    return { connection, revision: stream.length, pinnedSessions: new Map(this.pins.get(connectionId) ?? []) };
  }
}
