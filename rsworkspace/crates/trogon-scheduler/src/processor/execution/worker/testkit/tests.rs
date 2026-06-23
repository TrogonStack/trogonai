use futures::StreamExt;

    use super::*;

    #[test]
    fn debug_fmt_is_non_exhaustive() {
        let _ = format!("{:?}", InMemoryKv::new());
        let _ = format!("{:?}", InMemoryExecution::new());
    }

    #[tokio::test]
    async fn kv_get_create_update_and_keys_are_observable() {
        let kv = InMemoryKv::new();
        assert!(kv.get("missing".to_string()).await.unwrap().is_none());

        kv.create("v1.orders", Bytes::from_static(b"{}")).await.unwrap();
        assert_eq!(
            kv.get("v1.orders".to_string()).await.unwrap(),
            Some(Bytes::from_static(b"{}"))
        );

        let duplicate = kv.create("v1.orders", Bytes::from_static(b"x")).await;
        assert_eq!(duplicate.unwrap_err().kind(), kv::CreateErrorKind::AlreadyExists);

        kv.update("v1.orders", Bytes::from_static(b"v2"), 99).await.unwrap_err();
        kv.update("v1.orders", Bytes::from_static(b"v2"), 1).await.unwrap();

        let mut keys = kv.keys().await.unwrap();
        assert_eq!(keys.next().await.transpose().unwrap(), Some("v1.orders".to_string()));
        assert!(keys.next().await.is_none());
    }
