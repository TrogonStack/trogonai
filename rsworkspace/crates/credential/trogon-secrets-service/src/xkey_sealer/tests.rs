use nkeys::{KeyPair, XKey};

use super::*;

fn secret(value: &str) -> SecretString {
    SecretString::new(value).unwrap()
}

#[test]
fn refuses_a_seed_that_is_not_a_curve_seed() {
    let account_seed = KeyPair::new_account().seed().unwrap();

    assert_eq!(
        XkeySealer::from_seed(&secret(&account_seed)).unwrap_err(),
        XkeySealerError::NotACurveSeed
    );
    assert_eq!(
        XkeySealer::from_seed(&secret("not-a-seed")).unwrap_err(),
        XkeySealerError::NotACurveSeed
    );
    assert_eq!(
        XkeySealerError::NotACurveSeed.to_string(),
        "service xkey seed is not a curve seed"
    );
}

#[test]
fn seals_so_only_the_requester_opens_it_from_the_service() {
    let service = XKey::new();
    let requester = XKey::new();
    let sealer = XkeySealer::from_seed(&secret(&service.seed().unwrap())).unwrap();
    let recipient = RequesterXkey::parse(&requester.public_key()).unwrap();

    let sealed = sealer.seal(&secret("ghp_example"), &recipient).unwrap();

    let sender = XKey::from_public_key(&sealer.public_key()).unwrap();
    assert_eq!(requester.open(sealed.as_bytes(), &sender).unwrap(), b"ghp_example");
    assert!(XKey::new().open(sealed.as_bytes(), &sender).is_err());
    assert!(requester.open(sealed.as_bytes(), &XKey::new()).is_err());
}

#[test]
fn debug_shows_only_the_public_key() {
    let service = XKey::new();
    let seed = service.seed().unwrap();
    let sealer = XkeySealer::from_seed(&secret(&seed)).unwrap();

    let debug = format!("{sealer:?}");

    assert_eq!(debug, format!("XkeySealer({:?})", service.public_key()));
    assert!(!debug.contains(&seed));
}
