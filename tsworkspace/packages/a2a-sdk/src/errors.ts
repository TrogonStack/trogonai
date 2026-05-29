export type SdkErrorCode =
  | "InvalidAgentId"
  | "LookupFailed"
  | "ExchangeFailed"
  | "Forbidden"
  | "InvalidToken"
  | "ChainTooDeep"
  | "ChainLoop"
  | "TransportError"
  | "SerializationError"
  | "ConfigError"
  | "PurposeRequired";

export abstract class SdkError extends Error {
  abstract readonly code: SdkErrorCode;

  constructor(message: string, options?: { cause?: unknown }) {
    super(message, options);
    this.name = new.target.name;
  }
}

export class InvalidAgentId extends SdkError {
  readonly code = "InvalidAgentId" as const;
}

export class LookupFailed extends SdkError {
  readonly code = "LookupFailed" as const;
}

export class ExchangeFailed extends SdkError {
  readonly code = "ExchangeFailed" as const;
}

export class Forbidden extends SdkError {
  readonly code = "Forbidden" as const;
}

export class InvalidToken extends SdkError {
  readonly code = "InvalidToken" as const;
}

export class ChainTooDeep extends SdkError {
  readonly code = "ChainTooDeep" as const;
  readonly maxDepth: number;
  readonly actualDepth: number;

  constructor(maxDepth: number, actualDepth: number) {
    super(`act_chain depth ${actualDepth} exceeds max ${maxDepth}`);
    this.maxDepth = maxDepth;
    this.actualDepth = actualDepth;
  }
}

export class ChainLoop extends SdkError {
  readonly code = "ChainLoop" as const;
  readonly agentId: string;
  readonly wkl: string;

  constructor(agentId: string, wkl: string) {
    super(`act_chain loop at (${agentId}, ${wkl})`);
    this.agentId = agentId;
    this.wkl = wkl;
  }
}

export class TransportError extends SdkError {
  readonly code = "TransportError" as const;
}

export class SerializationError extends SdkError {
  readonly code = "SerializationError" as const;
}

export class ConfigError extends SdkError {
  readonly code = "ConfigError" as const;
}

export class PurposeRequired extends SdkError {
  readonly code = "PurposeRequired" as const;

  constructor() {
    super("purpose is required when the caller has multiple allowed purposes");
  }
}
