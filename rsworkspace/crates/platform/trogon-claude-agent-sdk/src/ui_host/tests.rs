use super::*;
use crate::constants::UI_SUBTYPES;

#[test]
fn a_ui_host_routes_each_subtype_to_its_own_method() {
    let handler: Arc<dyn UiHandler> = Arc::new(|_: UiRequest| async { Ok(None) });
    let host = UiHost {
        prompt_fill: Some(handler),
        ..UiHost::default()
    };
    for subtype in UI_SUBTYPES {
        assert_eq!(
            host.handler(subtype).is_some(),
            subtype == "ui_prompt_fill",
            "{subtype}"
        );
    }
    assert!(host.handler("ui_render").is_none());
    assert_eq!(
        format!("{host:?}"),
        "UiHost { copy: false, prompt_read: false, prompt_fill: true, prompt_paint: false, prompt_suggest: false, read_selection: false }"
    );
}

#[test]
fn a_ui_handler_trait_object_debugs_without_exposing_the_closure() {
    let handler: Arc<dyn UiHandler> = Arc::new(|_: UiRequest| async { Ok(None) });
    assert!(format!("{:?}", &*handler).starts_with("UiHandler"));
}
