import type {
  Command,
  CommandFinished,
  CommandCancelOptions,
  CommandCancellationError,
  RunCommandParams,
} from '../../src/index.ts';

declare const command: Command;
declare const failure: CommandCancellationError;
const options: CommandCancelOptions = { timeoutMs: 1_000 };
const cancelled: Promise<CommandFinished> = command.cancel(options);
const run: RunCommandParams = { cmd: 'sleep', signal: new AbortController().signal, cancellationTimeoutMs: 1_000 };
const pending: boolean = failure.cleanupPending;
void [cancelled, run, pending];
