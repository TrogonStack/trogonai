use super::*;

fn dirs(raw: &[&str]) -> Result<Vec<LiteralPath>, Box<dyn std::error::Error>> {
    Ok(raw.iter().map(|dir| LiteralPath::new(*dir)).collect::<Result<_, _>>()?)
}

fn get<'a>(env: &'a [(String, String)], name: &str) -> Option<&'a str> {
    env.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
}

#[test]
fn each_directory_gets_the_exact_path_and_the_glob_below_it() -> Result<(), Box<dyn std::error::Error>> {
    let mut env = vec![];
    apply(&mut env, &dirs(&["/workspace", "/srv/repo"])?);
    assert_eq!(get(&env, "GIT_CONFIG_COUNT"), Some("4"));
    let pairs = (0..4)
        .map(|i| -> Result<_, Box<dyn std::error::Error>> {
            Ok((
                get(&env, &format!("GIT_CONFIG_KEY_{i}")).ok_or("key")?,
                get(&env, &format!("GIT_CONFIG_VALUE_{i}")).ok_or("value")?,
            ))
        })
        .collect::<Result<Vec<_>, _>>()?;
    assert_eq!(
        pairs,
        vec![
            ("safe.directory", "/workspace"),
            ("safe.directory", "/workspace/*"),
            ("safe.directory", "/srv/repo"),
            ("safe.directory", "/srv/repo/*"),
        ]
    );
    Ok(())
}

#[test]
fn the_root_does_not_double_its_slash() -> Result<(), Box<dyn std::error::Error>> {
    let mut env = vec![];
    apply(&mut env, &dirs(&["/"])?);
    assert_eq!(get(&env, "GIT_CONFIG_VALUE_0"), Some("/"));
    assert_eq!(get(&env, "GIT_CONFIG_VALUE_1"), Some("/*"));
    Ok(())
}

#[test]
fn duplicates_collapse_and_a_long_list_becomes_one_wildcard() -> Result<(), Box<dyn std::error::Error>> {
    let mut env = vec![];
    apply(&mut env, &dirs(&["/a", "/a"])?);
    assert_eq!(get(&env, "GIT_CONFIG_COUNT"), Some("2"));

    let many: Vec<String> = (0..9).map(|i| format!("/d{i}")).collect();
    let refs: Vec<&str> = many.iter().map(String::as_str).collect();
    let mut env = vec![];
    apply(&mut env, &dirs(&refs)?);
    assert_eq!(get(&env, "GIT_CONFIG_COUNT"), Some("1"));
    assert_eq!(get(&env, "GIT_CONFIG_VALUE_0"), Some("*"));
    Ok(())
}

#[test]
fn an_existing_count_is_continued_and_zero_opts_out() -> Result<(), Box<dyn std::error::Error>> {
    let mut env = vec![
        ("GIT_CONFIG_COUNT".to_string(), "1".to_string()),
        ("GIT_CONFIG_KEY_0".to_string(), "user.name".to_string()),
        ("GIT_CONFIG_VALUE_0".to_string(), "x".to_string()),
    ];
    apply(&mut env, &dirs(&["/w"])?);
    assert_eq!(get(&env, "GIT_CONFIG_COUNT"), Some("3"));
    assert_eq!(get(&env, "GIT_CONFIG_KEY_0"), Some("user.name"));
    assert_eq!(get(&env, "GIT_CONFIG_VALUE_1"), Some("/w"));
    assert_eq!(get(&env, "GIT_CONFIG_VALUE_2"), Some("/w/*"));

    let mut env = vec![("GIT_CONFIG_COUNT".to_string(), "0".to_string())];
    apply(&mut env, &dirs(&["/w"])?);
    assert_eq!(env.len(), 1);
    Ok(())
}

#[test]
fn no_directories_leaves_the_environment_alone() -> Result<(), Box<dyn std::error::Error>> {
    let mut env = vec![("A".to_string(), "b".to_string())];
    apply(&mut env, &[]);
    assert_eq!(env.len(), 1);
    Ok(())
}
