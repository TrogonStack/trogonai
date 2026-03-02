use std::env;

use super::ReadEnv;

/// Zero-sized type — delegates to `std::env`.
pub struct SystemEnv;

impl ReadEnv for SystemEnv {
    #[inline]
    fn var(&self, key: &str) -> Result<String, env::VarError> {
        env::var(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_system_env_delegation() {
        let system_env = SystemEnv;
        let std_result = std::env::var("PATH");
        let provider_result = system_env.var("PATH");
        assert_eq!(std_result.is_ok(), provider_result.is_ok());
    }

    /// `var()` for a key that is guaranteed not to be set must return
    /// `Err(VarError::NotPresent)` — not an empty string, not a panic.
    #[test]
    fn var_returns_not_present_for_missing_variable() {
        let env = SystemEnv;
        let result = env.var("TROGON_GUARANTEED_ABSENT_VAR_9f3a82b1");
        assert!(
            matches!(result, Err(std::env::VarError::NotPresent)),
            "missing variable must return Err(NotPresent), got: {:?}",
            result
        );
    }

    #[test]
    fn test_generic_function_with_system_env() {
        fn get_value_or_default<E: ReadEnv>(env: &E, key: &str, default: &str) -> String {
            env.var(key).unwrap_or_else(|_| default.to_string())
        }

        let sys_env = SystemEnv;
        let result = get_value_or_default(&sys_env, "NONEXISTENT_VAR_12345", "default");
        assert_eq!(result, "default");
    }
}
