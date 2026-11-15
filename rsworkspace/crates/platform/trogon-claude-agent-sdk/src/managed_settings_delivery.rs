/// TypeScript `managedSettingsDelivery`: how `managedSettings` reaches the CLI. Python has no such option.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ManagedSettingsDelivery {
    /// A `--managed-settings` flag on the command line.
    #[default]
    Argv,
    /// The `initialize` request carries the settings and the CLI is started with
    /// `--managed-settings-in-initialize`. With no managed settings this behaves like `Argv`.
    Initialize,
}
