export interface RecordingLogger {
  readonly warnings: string[];
  debug(...args: unknown[]): void;
  info(...args: unknown[]): void;
  warn(...args: unknown[]): void;
  error(...args: unknown[]): void;
  setLevel(level: string): void;
  getLevel(): string;
  setName(name: string): void;
}

export function createRecordingLogger(): RecordingLogger {
  const warnings: string[] = [];
  return {
    warnings,
    debug() {},
    info() {},
    warn(...args) {
      warnings.push(args.map(String).join(' '));
    },
    error() {},
    setLevel() {},
    getLevel: () => 'info',
    setName() {},
  };
}

class SilentConsoleLogger {
  debug() {}
  info() {}
  warn() {}
  error() {}
  setLevel() {}
  getLevel() {
    return 'info';
  }
  setName() {}
}

class OutOfScopeInstallProvider {
  constructor() {
    throw new Error('OAuth installation is outside the receiver conformance scope');
  }
}

class InertWebClient {}

const fakeFunction = () => () => undefined;

export function createExternals(socketModeClient: unknown): Record<string, unknown> {
  return {
    '@slack/logger': {
      ConsoleLogger: SilentConsoleLogger,
      LogLevel: { DEBUG: 'debug', INFO: 'info', WARN: 'warn', ERROR: 'error' },
    },
    '@slack/oauth': { InstallProvider: OutOfScopeInstallProvider },
    '@slack/socket-mode': { SocketModeClient: socketModeClient },
    '@slack/web-api': { WebClient: InertWebClient },
    'path-to-regexp': { match: () => () => false },
    sinon: { fake: fakeFunction, spy: fakeFunction },
  };
}
