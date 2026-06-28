use super::*;

#[test]
fn test_system_env_delegation() {
    let system_env = SystemEnv;
    let std_result = std::env::var("PATH");
    let provider_result = system_env.var("PATH");
    assert_eq!(std_result.is_ok(), provider_result.is_ok());
}

#[test]
fn test_system_env_var_os_delegation() {
    let system_env = SystemEnv;
    assert_eq!(std::env::var_os("PATH").is_some(), system_env.var_os("PATH").is_some());
}

#[test]
fn test_system_env_vars_os_delegation() {
    let system_env = SystemEnv;
    assert_eq!(std::env::vars_os().count(), system_env.vars_os().len());
}

#[test]
fn test_system_env_vars_includes_path() {
    let system_env = SystemEnv;
    assert!(system_env.vars().iter().any(|(key, _)| key == "PATH"));
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
