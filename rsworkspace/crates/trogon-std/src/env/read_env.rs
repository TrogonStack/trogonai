use std::env;

/// # Thread Safety
///
/// Does **not** require `Send + Sync`. Add the bounds at your call site:
///
/// ```text
/// fn spawn_work<E: ReadEnv + Send + Sync + 'static>(env: Arc<E>) { … }
/// ```
pub trait ReadEnv {
    fn var(&self, key: &str) -> Result<String, env::VarError>;
}

#[cfg(test)]
mod tests {
    use super::ReadEnv;
    use crate::env::InMemoryEnv;

    #[test]
    fn set_var_is_returned_by_trait_method() {
        let env = InMemoryEnv::new();
        env.set("DATABASE_URL", "postgres://localhost/test");

        assert_eq!(
            env.var("DATABASE_URL").unwrap(),
            "postgres://localhost/test"
        );
    }

    #[test]
    fn missing_var_returns_not_present() {
        let env = InMemoryEnv::new();

        assert!(matches!(
            env.var("NOT_SET"),
            Err(std::env::VarError::NotPresent)
        ));
    }

    #[test]
    fn overwritten_var_returns_latest_value() {
        let env = InMemoryEnv::new();
        env.set("KEY", "first");
        env.set("KEY", "second");

        assert_eq!(env.var("KEY").unwrap(), "second");
    }

    #[test]
    fn removed_var_returns_not_present() {
        let env = InMemoryEnv::new();
        env.set("GONE", "value");
        env.remove("GONE");

        assert!(matches!(
            env.var("GONE"),
            Err(std::env::VarError::NotPresent)
        ));
    }

    #[test]
    fn trait_object_dispatch_works() {
        let env = InMemoryEnv::new();
        env.set("APP_NAME", "trogon");

        let dyn_env: &dyn ReadEnv = &env;
        assert_eq!(dyn_env.var("APP_NAME").unwrap(), "trogon");
    }

    #[test]
    fn generic_fn_uses_trait_bound() {
        fn require_var<E: ReadEnv>(env: &E, key: &str) -> String {
            env.var(key).expect("variable must be set")
        }

        let env = InMemoryEnv::new();
        env.set("REQUIRED", "present");

        assert_eq!(require_var(&env, "REQUIRED"), "present");
    }
}
