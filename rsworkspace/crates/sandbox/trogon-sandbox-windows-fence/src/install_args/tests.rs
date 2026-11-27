use super::*;
use crate::sublayer_guid::SublayerGuid;

#[test]
fn uninstall_args_order_is_stable() -> Result<(), Box<dyn std::error::Error>> {
    let opts = UninstallOptions {
        sublayer_guid: Some(SublayerGuid::new("g")?),
        keep_user: true,
        timeout_ms: None,
    };
    assert_eq!(
        uninstall_args(&opts),
        ["uninstall", "--sublayer-guid", "g", "--keep-user"]
    );
    assert_eq!(uninstall_args(&UninstallOptions::default()), ["uninstall"]);
    Ok(())
}
