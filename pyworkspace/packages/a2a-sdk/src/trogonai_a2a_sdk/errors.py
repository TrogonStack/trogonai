from __future__ import annotations

from typing import Literal


SdkErrorCode = Literal[
    "InvalidAgentId",
    "LookupFailed",
    "ExchangeFailed",
    "Forbidden",
    "InvalidToken",
    "ChainTooDeep",
    "ChainLoop",
    "TransportError",
    "SerializationError",
    "ConfigError",
    "PurposeRequired",
]


class SdkError(Exception):
    code: SdkErrorCode


class InvalidAgentId(SdkError):
    code = "InvalidAgentId"


class LookupFailed(SdkError):
    code = "LookupFailed"


class ExchangeFailed(SdkError):
    code = "ExchangeFailed"


class Forbidden(SdkError):
    code = "Forbidden"


class InvalidToken(SdkError):
    code = "InvalidToken"


class ChainTooDeep(SdkError):
    code = "ChainTooDeep"

    def __init__(self, max_depth: int, actual_depth: int) -> None:
        super().__init__(f"act_chain depth {actual_depth} exceeds max {max_depth}")
        self.max_depth = max_depth
        self.actual_depth = actual_depth


class ChainLoop(SdkError):
    code = "ChainLoop"

    def __init__(self, agent_id: str, wkl: str) -> None:
        super().__init__(f"act_chain loop at ({agent_id}, {wkl})")
        self.agent_id = agent_id
        self.wkl = wkl


class TransportError(SdkError):
    code = "TransportError"


class SerializationError(SdkError):
    code = "SerializationError"


class ConfigError(SdkError):
    code = "ConfigError"


class PurposeRequired(SdkError):
    code = "PurposeRequired"

    def __init__(self) -> None:
        super().__init__(
            "purpose is required when the caller has multiple allowed purposes"
        )
