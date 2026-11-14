//! Which lodash `mergeWith` call of the TypeScript SDK a merge models, which sets the stack it starts on.

use crate::node_managed_pass::ManagedPass;

/// A lodash `mergeWith` call with the settings customizer (`ao` upstream).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum NodeMergeCall {
    /// `WQe`: a settings source merged into the result.
    Source,
    /// `UP`: a managed settings document merged into the documents of its folder read before it, in an evaluation
    /// of the managed file tier.
    Folder(ManagedPass),
    /// `QP`: the parent slice, then the admin settings, merged onto a fresh object.
    Admin,
}
