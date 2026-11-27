/// Variables the payload inherits from the worker when the worker has them.
/// They locate the system directories and say how to find programs; none of
/// them carries a credential. Everything else of the worker's environment is
/// dropped.
pub(crate) const INHERITED: [&str; 9] = [
    "SystemRoot",
    "SystemDrive",
    "windir",
    "ComSpec",
    "PATHEXT",
    "OS",
    "PROCESSOR_ARCHITECTURE",
    "LANG",
    "TERM",
];
