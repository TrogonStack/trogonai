import { SandboxWsError } from './errors.ts';
import { nativeSnapshotDeleteResponseSchema, nativeSnapshotReferenceSchema } from './wire.ts';
import type { NativeSnapshotDeleteResponse, NativeSnapshotRef } from './wire.ts';

export function parseNativeSnapshotReference(value: unknown): NativeSnapshotRef {
  const parsed = nativeSnapshotReferenceSchema.safeParse(value);
  if (!parsed.success) throw new SandboxWsError('invalid_request', 'invalid provider-native snapshot reference');
  return parsed.data;
}

export function parseNativeSnapshotDeleteResponse(value: unknown): NativeSnapshotDeleteResponse {
  const parsed = nativeSnapshotDeleteResponseSchema.safeParse(value);
  if (!parsed.success) {
    throw new SandboxWsError('invalid_response', 'invalid provider-native snapshot deletion response', {
      outcomeUnknown: true,
    });
  }
  return parsed.data;
}
