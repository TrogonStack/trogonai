import type { DispatchOptions, EnvironmentUsage, EnvironmentsClient, StoredEnvironmentEvent } from '../client';
import type { EnvironmentCommand } from '../domain/commands';
import {
  decideEnvironment,
  foldEnvironment,
  type EnvironmentDecisionContext,
  type EnvironmentState,
} from '../domain/environment';
import { parseEnvironmentId } from '../domain/environment-id';
import type { EnvironmentEvent } from '../domain/events';
import { EnvironmentFault } from '../domain/faults';

export const DEMO_PROJECT = 'proj-demo';
const ACTOR = 'console-operator';

export interface InMemoryEnvironmentsOptions {
  readonly latencyMs?: number;
  readonly clock?: () => Date;
  readonly projectId?: string;
  readonly usage?: EnvironmentUsage;
}

export interface EnvironmentSeedWriter {
  environment(command: EnvironmentCommand, at: Date): Promise<EnvironmentState>;
}

const unusedEnvironments: EnvironmentUsage = { blockingVaults: () => Promise.resolve(0) };

function sleep(milliseconds: number): Promise<void> {
  return milliseconds > 0 ? new Promise((resolve) => setTimeout(resolve, milliseconds)) : Promise.resolve();
}

function newId(prefix: string): string {
  return `${prefix}_${crypto.randomUUID().replaceAll('-', '').slice(0, 12)}`;
}

export class InMemoryEnvironmentsClient implements EnvironmentsClient {
  private readonly streams = new Map<string, StoredEnvironmentEvent[]>();
  private readonly latencyMs: number;
  private readonly clock: () => Date;
  private readonly projectId: string;
  private readonly usage: EnvironmentUsage;
  private ready: Promise<void> = Promise.resolve();

  constructor(options: InMemoryEnvironmentsOptions = {}) {
    this.latencyMs = options.latencyMs ?? 0;
    this.clock = options.clock ?? (() => new Date());
    this.projectId = options.projectId ?? DEMO_PROJECT;
    this.usage = options.usage ?? unusedEnvironments;
  }

  seedWith(populate: (writer: EnvironmentSeedWriter) => Promise<void>): void {
    this.ready = populate({ environment: (command, at) => this.execute(command, {}, at, true) });
  }

  private async pause(): Promise<void> {
    await this.ready;
    await sleep(this.latencyMs);
  }

  async list(): Promise<readonly EnvironmentState[]> {
    await this.pause();
    return [...this.streams.keys()]
      .map((id) => this.stateOf(id))
      .filter((state): state is EnvironmentState => state !== undefined && state.lifecycle === 'ACTIVE');
  }

  async get(environmentId: string): Promise<EnvironmentState | undefined> {
    await this.pause();
    const state = this.stateOf(environmentId);
    return state?.lifecycle === 'ACTIVE' ? state : undefined;
  }

  async history(environmentId: string): Promise<readonly StoredEnvironmentEvent[]> {
    await this.pause();
    return [...(this.streams.get(environmentId) ?? [])];
  }

  async dispatch(command: EnvironmentCommand, options: DispatchOptions = {}): Promise<EnvironmentState> {
    await this.pause();
    return this.execute(command, options);
  }

  private async execute(
    command: EnvironmentCommand,
    options: DispatchOptions,
    at?: Date,
    trusted = false,
  ): Promise<EnvironmentState> {
    const key = command.environmentId;
    if (parseEnvironmentId(key) === undefined) {
      throw new EnvironmentFault('COMMAND_INVALID', 'The environment id is not valid.');
    }
    const stream = this.streams.get(key) ?? [];
    const commandId = options.commandId ?? crypto.randomUUID();
    if (stream.some((stored) => stored.commandId === commandId)) return this.stateOf(key)!;
    this.checkRevision(command, key, options, trusted);

    const inUse = command.case === 'deleteEnvironment' ? await this.usage.blockingVaults(command.environmentId) : 0;
    const context: EnvironmentDecisionContext = {
      now: at ?? this.clock(),
      projectId: this.projectId,
      newId,
      vaultsInUse: () => inUse,
    };
    const decision = decideEnvironment(this.stateOf(key), command, context);
    if (!decision.ok) throw decision.fault;
    this.streams.set(key, [...stream, ...decision.events.map((event) => this.store(event, commandId))]);
    return this.stateOf(key)!;
  }

  private checkRevision(command: EnvironmentCommand, key: string, options: DispatchOptions, trusted: boolean): void {
    if (command.case === 'createEnvironment' || trusted) return;
    const state = this.stateOf(key);
    if (state?.lifecycle !== 'ACTIVE') throw new EnvironmentFault('ENVIRONMENT_NOT_FOUND');
    if (options.expectedRevision === undefined || options.expectedRevision < 1) {
      throw new EnvironmentFault(
        'COMMAND_INVALID',
        'Commands on an existing environment require an expected revision.',
      );
    }
    if (options.expectedRevision !== state.revision) {
      throw new EnvironmentFault(
        'REVISION_CONFLICT',
        `The environment is at revision ${state.revision}, not ${options.expectedRevision}.`,
      );
    }
  }

  private store(event: EnvironmentEvent, commandId: string): StoredEnvironmentEvent {
    return { revision: event.revision, event, occurredAt: event.occurredAt, actor: ACTOR, commandId };
  }

  private stateOf(environmentId: string): EnvironmentState | undefined {
    const stream = this.streams.get(environmentId);
    return stream ? foldEnvironment(stream.map((stored) => stored.event)) : undefined;
  }
}
