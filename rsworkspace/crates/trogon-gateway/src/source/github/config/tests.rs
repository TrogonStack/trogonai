use super::*;

    #[test]
    fn github_webhook_secret_roundtrips() {
        let secret = GitHubWebhookSecret::new("super-secret").unwrap();
        assert_eq!(secret.as_str(), "super-secret");
    }

    #[test]
    fn github_webhook_secret_debug_redacts() {
        let secret = GitHubWebhookSecret::new("super-secret").unwrap();
        assert_eq!(format!("{secret:?}"), "GitHubWebhookSecret(****)");
    }
