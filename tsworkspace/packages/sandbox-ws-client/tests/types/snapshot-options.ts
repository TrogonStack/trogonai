import type {
  Sandbox,
  SandboxNativeSnapshotReceipt,
  SandboxSnapshotReceipt,
  SnapshotOptions,
} from '../../src/index.ts';

declare const sandbox: Sandbox;
declare const dynamicNative: boolean;
declare const options: SnapshotOptions;

const ordinary: Promise<SandboxSnapshotReceipt> = sandbox.snapshot();
const expiry: Promise<SandboxSnapshotReceipt> = sandbox.snapshot({ expiresInMs: 60_000 });
const falseNative: Promise<SandboxSnapshotReceipt> = sandbox.snapshot({ acceptNative: false, expiresInMs: 0 });
const trueNative: Promise<SandboxSnapshotReceipt | SandboxNativeSnapshotReceipt> = sandbox.snapshot({
  acceptNative: true,
  expiresInMs: 60_000,
});
const dynamic: Promise<SandboxSnapshotReceipt | SandboxNativeSnapshotReceipt> = sandbox.snapshot({
  acceptNative: dynamicNative,
});
const typedOptions: Promise<SandboxSnapshotReceipt | SandboxNativeSnapshotReceipt> = sandbox.snapshot(options);

// @ts-expect-error Native receipts cannot be treated as OCI-only receipts.
const nativeAsOci: Promise<SandboxSnapshotReceipt> = sandbox.snapshot({ acceptNative: true });

void [ordinary, expiry, falseNative, trueNative, dynamic, typedOptions, nativeAsOci];
