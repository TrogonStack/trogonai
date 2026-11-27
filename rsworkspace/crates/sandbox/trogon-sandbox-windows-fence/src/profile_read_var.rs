/// The variables a recorded grant can
/// offer a sandboxed command so a tool reads the real user's files in place.
/// Any other name `fence-helper` reports is dropped. `RustupHome` and `CargoHome`
/// name directories, which stay read-only to the command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ProfileReadVar {
    NpmConfigUserconfig,
    GitConfigGlobal,
    PipConfigFile,
    MavenArgs,
    RustupHome,
    CargoHome,
}

impl ProfileReadVar {
    pub const ALL: [Self; 6] = [
        Self::NpmConfigUserconfig,
        Self::GitConfigGlobal,
        Self::PipConfigFile,
        Self::MavenArgs,
        Self::RustupHome,
        Self::CargoHome,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::NpmConfigUserconfig => "NPM_CONFIG_USERCONFIG",
            Self::GitConfigGlobal => "GIT_CONFIG_GLOBAL",
            Self::PipConfigFile => "PIP_CONFIG_FILE",
            Self::MavenArgs => "MAVEN_ARGS",
            Self::RustupHome => "RUSTUP_HOME",
            Self::CargoHome => "CARGO_HOME",
        }
    }
}
