    use super::*;

    #[test]
    fn caller_https_auth_display_redacts() {
        let auth = CallerHttpsAuth::new("Bearer secret-token");
        assert_eq!(format!("{auth}"), "<redacted>");
        assert_eq!(auth.as_str(), "Bearer secret-token");
    }

    #[test]
    fn bridge_user_jwt_rejects_empty_and_non_three_segment() {
        assert!(BridgeUserJwt::new("").is_err());
        assert!(BridgeUserJwt::new("a.b").is_err());
        assert!(BridgeUserJwt::new("a..c").is_err());
        assert!(BridgeUserJwt::new("a.b.c").is_ok());
    }

    #[test]
    fn bridge_user_jwt_display_redacts() {
        let jwt = BridgeUserJwt::new("h.p.s").expect("valid shape");
        assert_eq!(format!("{jwt}"), "<redacted>");
    }

    #[test]
    fn debug_does_not_leak_secrets() {
        let auth = CallerHttpsAuth::new("Bearer secret-token");
        let auth_dbg = format!("{auth:?}");
        assert!(!auth_dbg.contains("secret-token"), "{auth_dbg}");
        assert!(auth_dbg.contains("<redacted>"), "{auth_dbg}");

        let jwt = BridgeUserJwt::new("hhh.ppp.sss").expect("valid shape");
        let jwt_dbg = format!("{jwt:?}");
        assert!(!jwt_dbg.contains("hhh"), "{jwt_dbg}");
        assert!(jwt_dbg.contains("<redacted>"), "{jwt_dbg}");
    }
