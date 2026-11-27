use super::*;

#[test]
fn a_command_runs_through_sh_with_every_argument_quoted() {
    let wrapped = shell_wrap(&["echo".to_owned(), "it's".to_owned()], None, &BTreeMap::new(), None);
    assert_eq!(wrapped, vec!["sh", "-c", "'echo' 'it'\\''s'"]);
}

#[test]
fn the_directory_and_environment_come_before_the_command() {
    let env = BTreeMap::from([("KEY".to_owned(), "va'lue".to_owned())]);
    let wrapped = shell_wrap(&["ls".to_owned()], Some("/work"), &env, None);
    assert_eq!(wrapped[2], "cd '/work' || exit 1; export KEY='va'\\''lue'; 'ls'");
}

#[test]
fn a_user_nests_the_script_inside_runuser() {
    let wrapped = shell_wrap(&["id".to_owned()], None, &BTreeMap::new(), Some("dev"));
    assert_eq!(wrapped[..2], ["sh", "-c"]);
    assert_eq!(wrapped[2], "runuser -u 'dev' -- sh -c ''\\''id'\\'''");
}
