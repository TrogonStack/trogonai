import { create, type MessageInitShape } from '@bufbuild/protobuf';
import { timestampFromDate } from '@bufbuild/protobuf/wkt';
import { CredentialEventSchema, type CredentialEvent } from '@trogonai/proto';

import type { DecisionContext } from './context';
import type { EventPointer } from './credential-state';
import { secretRefMessage, type SecretRefId } from './secret-ref';

export type CredentialFact = MessageInitShape<typeof CredentialEventSchema>['fact'];

export class Emitter {
  private readonly emitted: CredentialEvent[] = [];

  constructor(
    private readonly context: DecisionContext,
    private readonly secret: SecretRefId,
    private readonly baseRevision: number,
  ) {}

  emit(fact: CredentialFact): EventPointer {
    const revision = this.baseRevision + this.emitted.length + 1;
    const eventId = this.context.newId('evt');
    this.emitted.push(
      create(CredentialEventSchema, {
        secret: secretRefMessage(this.secret),
        projectId: this.context.projectId,
        eventId,
        revision: BigInt(revision),
        occurredAt: timestampFromDate(this.context.now),
        fact,
      }),
    );
    return { revision, eventId };
  }

  get events(): readonly CredentialEvent[] {
    return this.emitted;
  }
}

export function eventRef(secret: SecretRefId, pointer: EventPointer) {
  return { streamId: secret, revision: BigInt(pointer.revision), eventId: pointer.eventId };
}

export function fakeDigest(label: string): Uint8Array {
  const bytes = new Uint8Array(32);
  let hash = 0x811c9dc5;
  for (let index = 0; index < label.length; index += 1) {
    hash = Math.imul(hash ^ label.charCodeAt(index), 0x01000193) >>> 0;
  }
  for (let index = 0; index < bytes.length; index += 1) {
    hash = Math.imul(hash ^ (index + 0x9e), 0x01000193) >>> 0;
    bytes[index] = (hash >>> 8) & 0xff;
  }
  return bytes;
}
