use async_trait::async_trait;

use crate::K8sOpsError;

/// Reads the `USER` an image declares in its config, so the worker can tell a
/// non-root image from a root one before it sets the pod's security context.
#[async_trait]
pub trait ImageUserResolver: Send + Sync {
    /// The image's configured `USER`, or `None` when the image sets none.
    async fn image_user(&self, image: &str) -> Result<Option<String>, K8sOpsError>;
}

/// Whether an image `USER` (`uid`, `uid:gid`, or a name) is a numeric id other
/// than root. Kubernetes cannot verify `runAsNonRoot` for a user name, and
/// root or an unset user would be rejected, so only a non-zero numeric uid
/// counts.
pub(crate) fn image_user_is_non_root_uid(user: Option<&str>) -> bool {
    let uid = user.and_then(|u| u.split(':').next()).map(str::trim);
    matches!(uid.and_then(|u| u.parse::<u32>().ok()), Some(uid) if uid != 0)
}
