// edition:2024
#![allow(unused)]

// The domain crate is not a dependency of this fixture, so the shapes the lint
// keys on are restated here: the enum, and the trait that makes the const a
// declaration rather than an implementation detail.
enum WritePrecondition {
    Any,
    NoStream,
    StreamExists,
    StreamUnchanged,
}

trait Decider {
    const WRITE_PRECONDITION: WritePrecondition;
}

// A precondition that names an invariant: must NOT fire.
struct CreateSchedule;
impl Decider for CreateSchedule {
    const WRITE_PRECONDITION: WritePrecondition = WritePrecondition::NoStream;
}

struct PauseSchedule;
impl Decider for PauseSchedule {
    const WRITE_PRECONDITION: WritePrecondition = WritePrecondition::StreamUnchanged;
}

// Unconditional and unargued: the violation.
struct RecordHeartbeat;
impl Decider for RecordHeartbeat {
    const WRITE_PRECONDITION: WritePrecondition = WritePrecondition::Any;
}

// The same declaration through an import: same variant, must fire too.
use WritePrecondition::Any;

struct RecordImportedHeartbeat;
impl Decider for RecordImportedHeartbeat {
    const WRITE_PRECONDITION: WritePrecondition = Any;
}

// Unconditional and argued: must NOT fire.
struct RenameSession;
impl Decider for RenameSession {
    #[allow(
        weakened_write_precondition,
        reason = "a rename is a last-writer-wins fact that guards no invariant"
    )]
    const WRITE_PRECONDITION: WritePrecondition = WritePrecondition::Any;
}

// Silenced without an argument: must fire anyway.
struct RenameSessionUnargued;
impl Decider for RenameSessionUnargued {
    #[allow(weakened_write_precondition)]
    const WRITE_PRECONDITION: WritePrecondition = WritePrecondition::Any;
}

// A same-named const of an unrelated type is not a precondition declaration:
// must NOT fire.
enum Retry {
    Any,
    Never,
}

struct Unrelated;
impl Unrelated {
    const WRITE_PRECONDITION: Retry = Retry::Any;
}

// A precondition chosen at run time is outside this lint's reach, and saying so
// in a fixture keeps the omission deliberate rather than forgotten.
struct Dynamic;
impl Dynamic {
    fn precondition(&self) -> WritePrecondition {
        WritePrecondition::Any
    }
}

fn main() {}
