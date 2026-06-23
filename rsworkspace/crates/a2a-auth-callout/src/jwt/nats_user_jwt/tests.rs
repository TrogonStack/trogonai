    use super::*;
    use crate::permissions::IssuedPermissions;
    use crate::signing_key_source::KeyVersion;
    use nkeys::KeyPair;
    use serde_json::json;

    fn fixture_material() -> (MintingMaterial, String, KeyPair, KeyPair) {
        let issuer = KeyPair::new_account();
        let user = KeyPair::new_user();
        let issuer_seed = issuer.seed().expect("issuer seed");
        let material = MintingMaterial::new(
            KeyPair::from_seed(&issuer_seed).unwrap(),
            KeyVersion::new("test").unwrap(),
        );
        (material, issuer_seed, issuer, user)
    }

    #[test]
    fn minted_jwt_has_nats_header_and_verifies() {
        let (material, issuer_seed, issuer, user) = fixture_material();
        let caller_id = CallerId::new("caller1").unwrap();
        let claims = UserJwtClaims {
            kid: material.version().clone(),
            sub: ExternalSubject::new("alice").unwrap(),
            aud: AccountName::new("tenant-acme"),
            data: SpiceDbPrincipal(json!({"spicedb_subject": "user/alice"})),
            nats_permissions: IssuedPermissions::default_for_caller(&caller_id),
            caller_id,
        };
        let subject = UserJwtSubject::from_user_nkey(crate::wire::NkeyPublic::parse(user.public_key()).unwrap());
        let token = mint_nats_user_jwt(
            &claims,
            &material,
            &subject,
            UNIX_EPOCH + Duration::from_secs(2_000),
            Duration::from_secs(60),
        )
        .unwrap();

        let header: NatsJwtHeader = decode_segment(token.as_str().split('.').next().unwrap()).unwrap();
        assert_eq!(header.algorithm, HEADER_ALGORITHM);
        assert_eq!(header.kid.as_deref(), Some("test"));

        let payload: Value = decode_nats_user_payload(token.as_str()).unwrap();
        assert_eq!(payload["sub"], user.public_key());
        assert_eq!(payload["iss"], issuer.public_key());
        assert_eq!(payload["aud"], "tenant-acme");
        assert_eq!(payload["caller_id"], "caller1");
        assert_eq!(payload["nats"]["type"], "user");
        assert_eq!(payload["nats"]["pub"]["allow"][0], "a2a.gateway.>");

        let handle = SigningKeyHandle::new(
            material.version().clone(),
            super::super::SigningKey::from_seed(&issuer_seed).unwrap(),
        );
        let verified = verify_with_material(token.as_str(), &handle.minting_material()).unwrap();
        assert_eq!(verified.caller_id.as_str(), "caller1");
        assert_eq!(verified.aud.as_str(), "tenant-acme");
    }
