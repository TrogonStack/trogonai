use crate::ProtocolError;
use crate::{
    Arguments, CommandTimeout, CommandWire, Environment, ExecutionId, ProtocolVersion, ReleaseTimeout, SignalNumber,
    SignalRequestId, WorkingDirectory,
};

#[derive(Clone)]
pub enum Command {
    Init {
        version: ProtocolVersion,
        execution_id: ExecutionId,
        argv: Arguments,
        env: Environment,
        cwd: Option<WorkingDirectory>,
        timeout_ms: Option<CommandTimeout>,
        release_timeout_ms: Option<ReleaseTimeout>,
    },
    Release {
        execution_id: ExecutionId,
    },
    Signal {
        execution_id: ExecutionId,
        request_id: SignalRequestId,
        signal: SignalNumber,
    },
    Status {
        execution_id: ExecutionId,
    },
    Acknowledge {
        execution_id: ExecutionId,
    },
}

impl Command {
    pub fn execution_id(&self) -> &ExecutionId {
        match self {
            Self::Init { execution_id, .. }
            | Self::Release { execution_id }
            | Self::Signal { execution_id, .. }
            | Self::Status { execution_id }
            | Self::Acknowledge { execution_id } => execution_id,
        }
    }
}

impl TryFrom<CommandWire> for Command {
    type Error = ProtocolError;
    fn try_from(wire: CommandWire) -> Result<Self, Self::Error> {
        Ok(match wire {
            CommandWire::Init {
                version,
                execution_id,
                argv,
                env,
                cwd,
                timeout_ms,
                release_timeout_ms,
            } => Self::Init {
                version: ProtocolVersion::new(version)?,
                execution_id,
                argv: Arguments::new(argv)?,
                env: Environment::new(env)?,
                cwd: cwd.map(WorkingDirectory::new).transpose()?,
                timeout_ms: timeout_ms.map(CommandTimeout::new).transpose()?,
                release_timeout_ms: release_timeout_ms.map(ReleaseTimeout::new).transpose()?,
            },
            CommandWire::Release { execution_id } => Self::Release { execution_id },
            CommandWire::Signal {
                execution_id,
                request_id,
                signal,
            } => Self::Signal {
                execution_id,
                request_id: SignalRequestId::new(request_id)?,
                signal: SignalNumber::new(signal)?,
            },
            CommandWire::Status { execution_id } => Self::Status { execution_id },
            CommandWire::Acknowledge { execution_id } => Self::Acknowledge { execution_id },
        })
    }
}
