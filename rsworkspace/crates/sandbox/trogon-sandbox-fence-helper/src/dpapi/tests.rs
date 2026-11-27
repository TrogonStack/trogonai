use super::*;

#[test]
fn round_trip() -> Result<(), Box<dyn std::error::Error>> {
    let pt = b"the only gate is the file DACL";
    let ct = protect_machine(pt)?;
    assert_ne!(ct.as_slice(), pt);
    assert!(ct.len() > pt.len()); // header + MAC overhead
    let back = unprotect(&ct)?;
    assert_eq!(back.as_slice(), pt);
    Ok(())
}

#[test]
fn unprotect_rejects_garbage() {
    assert!(unprotect(b"not a dpapi blob").is_err());
}
