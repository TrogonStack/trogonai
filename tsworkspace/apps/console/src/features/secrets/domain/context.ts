import type { VaultLifecycle } from '@trogonai/proto';

import type { EnvironmentId } from '@/features/environments/domain/environment-id';

export interface VaultSnapshot {
  readonly lifecycle: VaultLifecycle;
  readonly environment: EnvironmentId;
}

export interface DecisionContext {
  readonly now: Date;
  readonly projectId: string;
  newId(prefix: string): string;
  vault(vaultId: string): VaultSnapshot | undefined;
  liveCredentialCount(vaultId: string): number;
}
