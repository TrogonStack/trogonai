import type { EnvironmentState } from './environment';
import type { EnvironmentId } from './environment-id';

export interface ActionAvailability {
  readonly available: boolean;
  readonly reason?: string;
}

export interface EnvironmentUsageSummary {
  readonly vaults: number;
  readonly credentials: number;
  readonly needsAttention: number;
}

export interface EnvironmentView {
  readonly environmentId: EnvironmentId;
  readonly revision: number;
  readonly displayName: string;
  readonly description: string;
  readonly builtIn: boolean;
  readonly createdAt: Date;
  readonly updatedAt: Date;
  readonly usage: EnvironmentUsageSummary;
  readonly actions: {
    readonly edit: ActionAvailability;
    readonly delete: ActionAvailability;
  };
}

export const noUsage: EnvironmentUsageSummary = { vaults: 0, credentials: 0, needsAttention: 0 };

function deletion(builtIn: boolean, vaults: number): ActionAvailability {
  if (builtIn) return { available: false, reason: 'A built-in environment cannot be deleted.' };
  if (vaults > 0) {
    return {
      available: false,
      reason: `${vaults} ${vaults === 1 ? 'vault uses' : 'vaults use'} this environment. Destroy ${vaults === 1 ? 'it' : 'them'} first.`,
    };
  }
  return { available: true };
}

export function toEnvironmentView(state: EnvironmentState, usage: EnvironmentUsageSummary = noUsage): EnvironmentView {
  return {
    environmentId: state.environmentId,
    revision: state.revision,
    displayName: state.displayName,
    description: state.description,
    builtIn: state.builtIn,
    createdAt: state.createdAt,
    updatedAt: state.updatedAt,
    usage,
    actions: { edit: { available: true }, delete: deletion(state.builtIn, usage.vaults) },
  };
}

export interface EnvironmentsOverview {
  readonly total: number;
  readonly builtIn: number;
  readonly custom: number;
  readonly inUse: number;
  readonly unused: number;
  readonly needsAttention: number;
}

export function summarizeEnvironments(environments: readonly EnvironmentView[]): EnvironmentsOverview {
  const inUse = environments.filter((environment) => environment.usage.vaults > 0).length;
  const builtIn = environments.filter((environment) => environment.builtIn).length;
  return {
    total: environments.length,
    builtIn,
    custom: environments.length - builtIn,
    inUse,
    unused: environments.length - inUse,
    needsAttention: environments.reduce((sum, environment) => sum + environment.usage.needsAttention, 0),
  };
}
