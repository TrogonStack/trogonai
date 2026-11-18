use super::*;

#[test]
fn a_source_reads_as_the_layer_it_names() {
    for (source, named) in [
        (SettingSource::Default, "default"),
        (SettingSource::File, "file"),
        (SettingSource::Environment, "environment"),
        (SettingSource::Flag, "flag"),
    ] {
        assert_eq!(format!("{source}"), named);
    }
}
