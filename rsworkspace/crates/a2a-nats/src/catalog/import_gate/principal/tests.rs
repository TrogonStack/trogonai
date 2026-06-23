    use super::*;

    #[test]
    fn imported_account_name_round_trip() {
        let n = ImportedAccountName::new("peer-acct");
        assert_eq!(n.as_str(), "peer-acct");
        assert_eq!(n.to_string(), "peer-acct");
    }
