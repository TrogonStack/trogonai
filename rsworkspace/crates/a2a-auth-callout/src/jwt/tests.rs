    use super::*;
    use crate::signing_key_source::{KeyVersion, SigningKeyHandle};
    use nkeys::KeyPair;
    use serde_json::json;

    #[test]
    fn mint_decodes_expected_claims() {
        let issuer = KeyPair::new_account();
        let issuer_seed = issuer.seed().expect("issuer seed");
        let user = KeyPair::new_user();
        let material = MintingMaterial::new(
            SigningKey::from_seed(&issuer_seed).unwrap().keypair().clone(),
            KeyVersion::new("test").unwrap(),
        );
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
        let token = claims
            .mint_for_test_ttl(&material, &subject, Duration::from_secs(60))
            .unwrap();
        let handle = SigningKeyHandle::new(material.version().clone(), SigningKey::from_seed(&issuer_seed).unwrap());
        let decoded = UserJwtClaims::verify_with_handles(token.as_str(), &[handle]).unwrap();
        // `sub` round-trips the caller-supplied ExternalSubject ("alice"),
        // independent of the SpiceDB principal which can carry a different
        // shape ("user/alice"). See nats_user_jwt::ext_sub.
        assert_eq!(decoded.sub.as_str(), "alice");
        assert_eq!(decoded.aud.as_str(), "tenant-acme");
        assert_eq!(decoded.caller_id.as_str(), "caller1");
        assert_eq!(decoded.data.spicedb_subject().unwrap().as_str(), "user/alice");
    }

    #[test]
    fn mint_rejects_wrong_verification_key() {
        let issuer_a = KeyPair::new_account();
        let issuer_a_seed = issuer_a.seed().expect("issuer seed");
        let issuer_b = KeyPair::new_account();
        let issuer_b_seed = issuer_b.seed().expect("issuer b seed");
        let user = KeyPair::new_user();
        let material = MintingMaterial::new(
            SigningKey::from_seed(&issuer_a_seed).unwrap().keypair().clone(),
            KeyVersion::new("test").unwrap(),
        );
        let caller_id = CallerId::new("cid").unwrap();
        let claims = UserJwtClaims {
            kid: material.version().clone(),
            sub: ExternalSubject::new("s").unwrap(),
            aud: AudienceAccount::new("a"),
            data: SpiceDbPrincipal(json!({})),
            nats_permissions: IssuedPermissions::default_for_caller(&caller_id),
            caller_id,
        };
        let subject = UserJwtSubject::from_user_nkey(crate::wire::NkeyPublic::parse(user.public_key()).unwrap());
        let token = claims
            .mint_for_test_ttl(&material, &subject, Duration::from_secs(60))
            .unwrap();
        let wrong = SigningKeyHandle::new(
            KeyVersion::new("test").unwrap(),
            SigningKey::from_seed(&issuer_b_seed).unwrap(),
        );
        assert!(UserJwtClaims::verify_with_handles(token.as_str(), &[wrong]).is_err());
    }

    #[test]
    fn caller_id_rejects_dots() {
        assert!(CallerId::new("a.b").unwrap_err().to_string().contains("caller_id"));
    }

    #[test]
    fn external_subject_requires_non_empty() {
        assert!(
            ExternalSubject::new("")
                .unwrap_err()
                .to_string()
                .contains("external subject")
        );
    }

    #[test]
    fn spicedb_principal_prefers_custom_claim() {
        let v = json!({ "sub": "x", "spicedb_principal": { "kind": "special" } });
        let p = spicedb_principal_from_oidc_claims(&v);
        assert_eq!(p.0["kind"], "special");
    }

    #[test]
    fn spicedb_subject_accessor_reads_claim() {
        let p = SpiceDbPrincipal::new("user/alice");
        assert_eq!(p.spicedb_subject().unwrap().as_str(), "user/alice");
    }

    #[test]
    fn spicedb_subject_accessor_absent_when_missing_or_empty() {
        assert!(SpiceDbPrincipal(json!({})).spicedb_subject().is_none());
        assert!(
            SpiceDbPrincipal(json!({"spicedb_subject": ""}))
                .spicedb_subject()
                .is_none()
        );
    }
