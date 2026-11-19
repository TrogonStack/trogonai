import type {
  NativeSnapshotDeleteResponse,
  NativeSnapshotRef,
  Sandbox,
  SandboxClientApi,
  SandboxWsClient,
} from '../../src/index.ts';

declare const client: SandboxWsClient;
declare const api: SandboxClientApi;
declare const reference: NativeSnapshotRef;
declare const sandboxType: typeof Sandbox;

const direct: Promise<NativeSnapshotDeleteResponse> = client.nativeSnapshotDelete(reference);
const adapter: Promise<NativeSnapshotDeleteResponse> = api.nativeSnapshotDelete(reference);
const retired: Promise<boolean> = sandboxType.deleteNativeSnapshot(api, reference);

// @ts-expect-error OCI digests use snapshotDelete instead.
client.nativeSnapshotDelete('sha256:abc');
// @ts-expect-error Native deletion does not pretend a byte count is known.
const bytes: NativeSnapshotDeleteResponse = { outcome: 'deleted', bytes: 12 };

void [direct, adapter, retired, bytes];
