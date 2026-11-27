use k8s_openapi::apimachinery::pkg::apis::meta::v1::Status;

/// Translates the kube exec API's terminal [`Status`] object into a process
/// exit code: `Success` (no status received counts as success too, matching
/// the attached process having already joined without error) is `0`; a
/// `NonZeroExitCode` failure carries the real code in an `ExitCode` cause;
/// any other failure reason has no numeric code, so it's reported as `1`.
pub(crate) fn exit_code_from_status(status: Option<&Status>) -> i32 {
    let Some(status) = status else {
        return 0;
    };
    if status.status.as_deref() != Some("Failure") {
        return 0;
    }
    status
        .details
        .as_ref()
        .and_then(|d| d.causes.as_ref())
        .into_iter()
        .flatten()
        .find(|cause| cause.reason.as_deref() == Some("ExitCode"))
        .and_then(|cause| cause.message.as_deref())
        .and_then(|msg| msg.parse::<i32>().ok())
        .unwrap_or(1)
}
