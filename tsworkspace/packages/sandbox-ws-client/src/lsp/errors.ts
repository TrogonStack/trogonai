import { SandboxWsError } from '../errors.ts';

export class LspProtocolError extends SandboxWsError {
  constructor(message: string) {
    super('lsp_protocol', message);
    this.name = 'LspProtocolError';
  }
}

export class LspTimeoutError extends SandboxWsError {
  constructor(method: string, timeoutMs: number) {
    super('lsp_timeout', `LSP request ${method} timed out after ${timeoutMs}ms`);
    this.name = 'LspTimeoutError';
  }
}

export class LspResponseError extends SandboxWsError {
  readonly rpcCode: number;
  readonly data: unknown;

  constructor(rpcCode: number, message: string, data?: unknown) {
    super('lsp_response', message);
    this.name = 'LspResponseError';
    this.rpcCode = rpcCode;
    this.data = data;
  }
}

export class LspNotRunningError extends SandboxWsError {
  constructor(message: string) {
    super('lsp_not_running', message);
    this.name = 'LspNotRunningError';
  }
}

export class LspServerExitedError extends SandboxWsError {
  readonly exitCode: number | undefined;

  constructor(exitCode: number | undefined) {
    super('lsp_server_exited', `language server exited${exitCode === undefined ? '' : ` with code ${exitCode}`}`);
    this.name = 'LspServerExitedError';
    this.exitCode = exitCode;
  }
}
