use super::*;

#[test]
fn plaintext_material_exposes_its_value() {
    let material = SecretMaterial::plaintext(SecretString::new("Bot token").unwrap());

    assert_eq!(material.as_plaintext().unwrap().as_str(), "Bot token");
}

#[test]
fn verifier_material_never_exposes_plaintext() {
    let material = SecretMaterial::Verifier(SecretVerifier::new("verifier").unwrap());

    assert!(material.as_plaintext().is_none());
}
