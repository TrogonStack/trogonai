import type { ProjectState } from '@trogonai/proto';

import { nodeIdOf, type NodeId } from '@/features/hierarchy/domain/node-id';

import type {
  DispatchOptions,
  ProjectRecord,
  ProjectResources,
  ProjectResourceSummary,
  ProjectsClient,
  StoredProjectEvent,
  WorkspaceNode,
  WorkspaceNodeDirectory,
} from '../client';
import { commandProjectId, type ProjectCommand } from '../domain/commands';
import { decide, fold, initialState, presentProject } from '../domain/decider';
import { invalidProject, ProjectFault } from '../domain/faults';
import { parseProjectId, type ProjectId } from '../domain/project-id';

const ACTOR = 'console-operator';

export interface InMemoryProjectsOptions {
  readonly latencyMs?: number;
  readonly clock?: () => Date;
  readonly nodes?: WorkspaceNodeDirectory;
  readonly resources?: ProjectResources;
}

export interface ProjectSeedWriter {
  dispatch(command: ProjectCommand, at: Date): Promise<ProjectRecord>;
}

const emptyResources: ProjectResources = {
  summarize: () => Promise.resolve({ vaults: 0, credentials: 0, environments: 0 }),
};

function sleep(milliseconds: number): Promise<void> {
  return milliseconds > 0 ? new Promise((resolve) => setTimeout(resolve, milliseconds)) : Promise.resolve();
}

export class InMemoryProjectsClient implements ProjectsClient {
  private readonly streams = new Map<string, StoredProjectEvent[]>();
  private readonly latencyMs: number;
  private readonly clock: () => Date;
  private readonly nodes: WorkspaceNodeDirectory | undefined;
  private readonly projectResources: ProjectResources;
  private ready: Promise<void> = Promise.resolve();

  constructor(options: InMemoryProjectsOptions = {}) {
    this.latencyMs = options.latencyMs ?? 0;
    this.clock = options.clock ?? (() => new Date());
    this.nodes = options.nodes;
    this.projectResources = options.resources ?? emptyResources;
  }

  seedWith(populate: (writer: ProjectSeedWriter) => Promise<void>): void {
    this.ready = populate({ dispatch: (command, at) => this.execute(command, {}, at, true) });
  }

  private async pause(): Promise<void> {
    await this.ready;
    await sleep(this.latencyMs);
  }

  async list(): Promise<readonly ProjectRecord[]> {
    await this.pause();
    return [...this.streams.keys()]
      .map((id) => this.record(id))
      .filter((record): record is ProjectRecord => record !== undefined);
  }

  async get(projectId: string): Promise<ProjectRecord | undefined> {
    await this.pause();
    return this.record(projectId);
  }

  async history(projectId: string): Promise<readonly StoredProjectEvent[]> {
    await this.pause();
    return [...(this.streams.get(projectId) ?? [])];
  }

  async parentCandidates(): Promise<readonly WorkspaceNode[]> {
    await this.pause();
    return (await this.nodes?.workspaceNodes()) ?? [];
  }

  async attachedTo(nodeId: NodeId): Promise<number> {
    await this.pause();
    return [...this.streams.keys()].filter((id) => this.record(id)?.project.parent?.value === nodeId).length;
  }

  async resources(projectId: ProjectId): Promise<ProjectResourceSummary> {
    await this.pause();
    return this.projectResources.summarize(projectId);
  }

  async dispatch(command: ProjectCommand, options: DispatchOptions = {}): Promise<ProjectRecord> {
    await this.pause();
    if (command.case === 'create') await this.admitParent(command);
    return this.execute(command, options);
  }

  private async admitParent(command: Extract<ProjectCommand, { case: 'create' }>): Promise<void> {
    const parent = nodeIdOf(command.value.parent);
    if (parent === '' || !this.nodes) return;
    const nodes = await this.nodes.workspaceNodes();
    if (!nodes.some((node) => node.nodeId === parent)) {
      throw invalidProject('The parent must be a workspace or a node below it, never the tenant root.');
    }
  }

  private async execute(
    command: ProjectCommand,
    options: DispatchOptions,
    at?: Date,
    trusted = false,
  ): Promise<ProjectRecord> {
    const key = commandProjectId(command);
    if (parseProjectId(key) === undefined) throw invalidProject('The project id is not valid.');
    const stream = this.streams.get(key) ?? [];
    const commandId = options.commandId ?? crypto.randomUUID();

    if (stream.some((stored) => stored.commandId === commandId)) return this.record(key)!;

    if (command.case !== 'create' && stream.length > 0 && !trusted) {
      if (options.expectedRevision === undefined || options.expectedRevision < 1) {
        throw new ProjectFault('EXPECTED_REVISION_REQUIRED');
      }
      if (options.expectedRevision !== stream.length) {
        throw new ProjectFault(
          'REVISION_MISMATCH',
          `The project is at revision ${stream.length}, not ${options.expectedRevision}.`,
        );
      }
    }

    const decision = decide(this.stateOf(stream), command);
    if (!decision.ok) throw decision.fault;

    const occurredAt = at ?? this.clock();
    const appended = decision.events.map<StoredProjectEvent>((event, index) => ({
      revision: stream.length + index + 1,
      event,
      occurredAt,
      actor: ACTOR,
      commandId,
    }));
    this.streams.set(key, [...stream, ...appended]);
    return this.record(key)!;
  }

  private stateOf(stream: readonly StoredProjectEvent[]): ProjectState {
    return stream.length === 0 ? initialState() : fold(stream.map((stored) => stored.event));
  }

  private record(projectId: string): ProjectRecord | undefined {
    const stream = this.streams.get(projectId);
    if (!stream || stream.length === 0) return undefined;
    const project = presentProject(this.stateOf(stream));
    if (!project) return undefined;
    return {
      project,
      revision: stream.length,
      createdAt: stream[0]!.occurredAt,
      updatedAt: stream[stream.length - 1]!.occurredAt,
    };
  }
}
