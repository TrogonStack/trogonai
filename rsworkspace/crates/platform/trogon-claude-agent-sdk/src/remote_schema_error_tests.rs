use super::*;
use crate::{PythonCodePoint, PythonCodePointError};

#[test]
fn keeps_the_python_class_and_text() {
    let error = RemoteSchemaError::new("URLError", "<urlopen error no host given>");
    assert_eq!(error.class(), "URLError");
    assert_eq!(error.text().as_str(), Some("<urlopen error no host given>"));
    assert_eq!(error.to_string(), "URLError: <urlopen error no host given>");
    assert_eq!(error.clone(), error);
}

#[test]
fn keeps_a_lone_surrogate_and_displays_it_as_the_replacement_character() -> Result<(), PythonCodePointError> {
    let mut text = PythonString::from("unknown url type: x");
    text.push(PythonCodePoint::try_from(0xDCFFu32)?);
    let error = RemoteSchemaError::new("URLError", text.clone());
    assert_eq!(error.text(), &text);
    assert_eq!(error.text().as_str(), None);
    assert_eq!(error.to_string(), "URLError: unknown url type: x\u{fffd}");
    Ok(())
}
