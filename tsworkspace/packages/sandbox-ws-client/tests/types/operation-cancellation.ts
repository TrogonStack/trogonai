import type {
  CancelOperationResponse,
  CreateSandboxOptions,
  GetOperationResponse,
  OperationCapabilities,
  OperationCancellationError,
  SandboxClientApi,
} from '../../src/index.ts';

declare const client: SandboxClientApi;
declare const failure: OperationCancellationError;
const capabilities: Promise<OperationCapabilities> = client.operationCapabilities();
const cancellation: Promise<CancelOperationResponse> = client.cancelOperation('operation-1');
const compensation: Promise<CancelOperationResponse> = client.cancelOperation('operation-1', {
  compensateCreated: true,
});
const status: Promise<GetOperationResponse> = client.getOperation('operation-1');
const options: Partial<CreateSandboxOptions> = { signal: new AbortController().signal, cancellationTimeoutMs: 1_000 };
const known: string = failure.operationId;
void [capabilities, cancellation, compensation, status, options, known];
