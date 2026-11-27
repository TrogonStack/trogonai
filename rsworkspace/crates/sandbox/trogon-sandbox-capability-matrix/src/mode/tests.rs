use super::*;

#[test]
fn no_arguments_check() -> Result<(), MatrixError> {
    assert_eq!(Mode::from_args(Vec::new())?, Mode::Check);
    Ok(())
}

#[test]
fn write_flag_writes() -> Result<(), MatrixError> {
    assert_eq!(Mode::from_args(vec!["--write".to_owned()])?, Mode::Write);
    Ok(())
}

#[test]
fn unknown_arguments_are_refused() {
    assert!(matches!(
        Mode::from_args(vec!["--wrte".to_owned()]),
        Err(MatrixError::UnknownArgument(arg)) if arg == "--wrte"
    ));
}
