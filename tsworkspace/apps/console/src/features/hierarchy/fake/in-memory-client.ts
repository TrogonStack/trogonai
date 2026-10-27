import type {
  DispatchOptions,
  HierarchyClient,
  LabelDirectory,
  NodeAttachments,
  NodeLabelEntry,
  StoredHierarchyEvent,
} from '../client';
import type { HierarchyCommand } from '../domain/commands';
import { decideHierarchy, foldHierarchy, type HierarchyDecisionContext } from '../domain/decider';
import type { HierarchyEvent } from '../domain/events';
import { HierarchyFault } from '../domain/faults';
import { parseNodeId, type NodeId } from '../domain/node-id';
import type { NodeName } from '../domain/node-name';
import { hasNode, type TreeState } from '../domain/tree';
import { InMemoryLabelDirectory } from './label-directory';

const ACTOR = 'console-operator';

export interface InMemoryHierarchyOptions {
  readonly latencyMs?: number;
  readonly clock?: () => Date;
  readonly attachments?: NodeAttachments;
  readonly labels?: LabelDirectory;
}

export interface HierarchySeedWriter {
  hierarchy(command: HierarchyCommand, at: Date): Promise<TreeState>;
  label(nodeId: NodeId, name: NodeName): Promise<void>;
}

const noAttachments: NodeAttachments = { attachedTo: () => Promise.resolve(0) };

function sleep(milliseconds: number): Promise<void> {
  return milliseconds > 0 ? new Promise((resolve) => setTimeout(resolve, milliseconds)) : Promise.resolve();
}

function newId(prefix: string): string {
  return `${prefix}_${crypto.randomUUID().replaceAll('-', '').slice(0, 12)}`;
}

export class InMemoryHierarchyClient implements HierarchyClient {
  private stream: readonly StoredHierarchyEvent[] = [];
  private readonly latencyMs: number;
  private readonly clock: () => Date;
  private readonly attachments: NodeAttachments;
  private readonly directory: LabelDirectory;
  private seed: () => Promise<void> = () => Promise.resolve();
  private ready: Promise<void> | undefined;

  constructor(options: InMemoryHierarchyOptions = {}) {
    this.latencyMs = options.latencyMs ?? 0;
    this.clock = options.clock ?? (() => new Date());
    this.attachments = options.attachments ?? noAttachments;
    this.directory = options.labels ?? new InMemoryLabelDirectory();
  }

  seedWith(populate: (writer: HierarchySeedWriter) => Promise<void>): void {
    this.seed = () =>
      populate({
        hierarchy: (command, at) => this.execute(command, {}, at, true),
        label: (nodeId, name) => this.directory.set(nodeId, name),
      });
  }

  private async pause(): Promise<void> {
    this.ready ??= this.seed();
    await this.ready;
    await sleep(this.latencyMs);
  }

  async tree(): Promise<TreeState> {
    await this.pause();
    return this.current();
  }

  async history(): Promise<readonly StoredHierarchyEvent[]> {
    await this.pause();
    return this.stream;
  }

  async labels(): Promise<readonly NodeLabelEntry[]> {
    await this.pause();
    return this.directory.entries();
  }

  async setLabel(nodeId: NodeId, name: NodeName): Promise<void> {
    await this.pause();
    if (!hasNode(this.current(), nodeId)) throw new HierarchyFault('NODE_NOT_FOUND');
    await this.directory.set(nodeId, name);
  }

  async dispatch(command: HierarchyCommand, options: DispatchOptions = {}): Promise<TreeState> {
    await this.pause();
    return this.execute(command, options);
  }

  private async execute(
    command: HierarchyCommand,
    options: DispatchOptions,
    at?: Date,
    trusted = false,
  ): Promise<TreeState> {
    if (parseNodeId(command.nodeId) === undefined) {
      throw new HierarchyFault('COMMAND_INVALID', 'The node id is not valid.');
    }
    const commandId = options.commandId ?? crypto.randomUUID();
    if (this.stream.some((stored) => stored.commandId === commandId)) return this.current();
    if (!trusted) this.checkRevision(options);

    const attached = command.case === 'removeNode' ? await this.attachments.attachedTo(command.nodeId) : 0;
    const context: HierarchyDecisionContext = { now: at ?? this.clock(), newId, attachedTo: () => attached };
    const decision = decideHierarchy(this.current(), command, context);
    if (!decision.ok) throw decision.fault;
    this.stream = [...this.stream, ...decision.events.map((event) => this.store(event, commandId))];
    return this.current();
  }

  private checkRevision(options: DispatchOptions): void {
    const { revision } = this.current();
    if (options.expectedRevision === undefined || options.expectedRevision < 0) {
      throw new HierarchyFault('COMMAND_INVALID', 'Commands on the tree require an expected revision.');
    }
    if (options.expectedRevision !== revision) {
      throw new HierarchyFault(
        'REVISION_CONFLICT',
        `The tree is at revision ${revision}, not ${options.expectedRevision}.`,
      );
    }
  }

  private store(event: HierarchyEvent, commandId: string): StoredHierarchyEvent {
    return { revision: event.revision, event, occurredAt: event.occurredAt, actor: ACTOR, commandId };
  }

  private current(): TreeState {
    return foldHierarchy(this.stream.map((stored) => stored.event));
  }
}
