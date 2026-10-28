import type {
  CredentialBinding,
  ProviderConnection,
  ProviderConnectionEvent,
  ProviderValidationRef,
} from '@trogonai/proto';

import type { ProviderCommand } from './domain/decider';

export interface ProviderConnectionRecord {
  readonly connection: ProviderConnection;
  readonly revision: number;
  readonly pinnedSessions: ReadonlyMap<string, number>;
}

export interface StoredEvent {
  readonly revision: number;
  readonly event: ProviderConnectionEvent;
  readonly occurredAt: Date;
  readonly actor: string;
  readonly commandId: string;
}

export interface DispatchOptions {
  readonly expectedRevision?: number;
  readonly commandId?: string;
}

export interface ValidationRequest {
  readonly connectionId: string;
  readonly binding: CredentialBinding;
}

export interface ProviderConnectionsClient {
  list(): Promise<readonly ProviderConnectionRecord[]>;
  get(connectionId: string): Promise<ProviderConnectionRecord | undefined>;
  history(connectionId: string): Promise<readonly StoredEvent[]>;
  requestValidation(request: ValidationRequest): Promise<ProviderValidationRef>;
  dispatch(command: ProviderCommand, options?: DispatchOptions): Promise<ProviderConnectionRecord>;
  endPinnedSessions(connectionId: string, bindingId: string): Promise<void>;
}
