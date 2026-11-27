use super::*;
use crate::isolation::seatbelt::test_support::{entry, glob, lp};

fn identity() -> PathMapper {
    PathMapper::new(vec![])
}

fn regexes(filters: &[Filter]) -> Vec<String> {
    filters
        .iter()
        .filter_map(|f| match f {
            Filter::Regex(s) => Some(s.as_str().to_string()),
            _ => None,
        })
        .collect()
}

fn matches_source(source: &str, text: &str) -> bool {
    regex::Regex::new(source).is_ok_and(|re| re.is_match(text))
}

#[test]
fn guest_paths_are_rewritten_to_the_host_workspace() -> Result<(), Box<dyn std::error::Error>> {
    let mapper = PathMapper::new(vec![(lp("/workspace")?, lp("/host/ws")?)]);
    assert_eq!(mapper.map(&lp("/workspace/a/b")?)?, lp("/host/ws/a/b")?);
    assert_eq!(mapper.map(&lp("/workspace")?)?, lp("/host/ws")?);
    assert_eq!(mapper.map(&lp("/etc/hosts")?)?, lp("/etc/hosts")?);
    assert_eq!(mapper.map(&lp("/workspaces/a")?)?, lp("/workspaces/a")?);
    Ok(())
}

#[test]
fn the_longest_mount_wins() -> Result<(), Box<dyn std::error::Error>> {
    let mapper = PathMapper::new(vec![
        (lp("/workspace")?, lp("/host/ws")?),
        (lp("/workspace/data")?, lp("/host/data")?),
    ]);
    assert_eq!(mapper.map(&lp("/workspace/data/x")?)?, lp("/host/data/x")?);
    Ok(())
}

#[test]
fn symlinked_locations_get_both_spellings() -> Result<(), Box<dyn std::error::Error>> {
    let spellings = identity().host_spellings(&lp("/tmp/not-there/x")?)?;
    assert_eq!(spellings[0], lp("/tmp/not-there/x")?);
    assert!(spellings.contains(&lp("/private/tmp/not-there/x")?));
    Ok(())
}

#[test]
fn a_literal_with_brackets_is_a_subpath_never_a_regex() -> Result<(), Box<dyn std::error::Error>> {
    let entries = [entry("/work/a[b/c]d")?];
    let region = Region::translate(&entries, &identity())?;
    assert_eq!(region.filters, vec![Filter::subpath("/work/a[b/c]d")?]);
    Ok(())
}

#[test]
fn an_absent_deny_spelled_with_a_slash_run_is_the_canonical_subpath() -> Result<(), Box<dyn std::error::Error>> {
    let entries = [entry("/work/not-there/a//b")?];
    let region = Region::translate(&entries, &identity())?;
    assert_eq!(region.filters, vec![Filter::subpath("/work/not-there/a/b")?]);
    Ok(())
}

#[test]
fn a_deny_glob_spelled_with_a_slash_run_compiles_like_the_clean_one() -> Result<(), Box<dyn std::error::Error>> {
    let translate = |raw: &str| -> Result<Vec<String>, Box<dyn std::error::Error>> {
        let entries = [glob(raw)?];
        Ok(regexes(&Region::translate(&entries, &identity())?.filters))
    };
    let spelled = translate("/work/not-there/a//*.pem")?;
    assert_eq!(spelled, translate("/work/not-there/a/*.pem")?);
    assert!(!spelled[0].contains("//"));
    Ok(())
}

#[test]
fn a_glob_under_a_bracketed_directory_keeps_the_directory_literal() -> Result<(), Box<dyn std::error::Error>> {
    let entries = [glob("/work/a[b/**/*.key")?];
    let region = Region::translate(&entries, &identity())?;
    let source = &regexes(&region.filters)[0];
    assert!(source.starts_with(r"^/work/a\[b/"), "{source}");
    assert!(matches_source(source, "/work/a[b/x/y.key"));
    assert!(matches_source(source, "/work/a[b/y.key"));
    assert!(!matches_source(source, "/work/ab/y.key"));
    assert_eq!(region.anchors, vec![lp("/work/a[b")?]);
    Ok(())
}

fn linked_dir() -> Result<(tempfile::TempDir, LiteralPath, LiteralPath), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let real = dir.path().canonicalize()?;
    std::fs::create_dir(real.join("target"))?;
    std::os::unix::fs::symlink(real.join("target"), real.join("link"))?;
    let root = lp(real.to_str().ok_or("a non-UTF-8 temp dir")?)?;
    let link = lp(&format!("{root}/link"))?;
    Ok((dir, root, link))
}

#[test]
fn a_deny_is_known_by_where_its_link_leads_too() -> Result<(), Box<dyn std::error::Error>> {
    let (_dir, root, link) = linked_dir()?;
    let spellings = identity().host_spellings(&link)?;
    assert_eq!(spellings, vec![link, lp(&format!("{root}/target"))?]);
    Ok(())
}

#[test]
fn an_allowed_name_keeps_its_last_component_as_written() -> Result<(), Box<dyn std::error::Error>> {
    let (_dir, _root, link) = linked_dir()?;
    assert_eq!(identity().name_spellings(&link)?, vec![link]);
    Ok(())
}

#[test]
fn an_allowed_name_resolves_the_links_on_the_way() -> Result<(), Box<dyn std::error::Error>> {
    let (_dir, root, _link) = linked_dir()?;
    let through = lp(&format!("{root}/link/inner"))?;
    let spellings = identity().name_spellings(&through)?;
    assert_eq!(spellings, vec![through, lp(&format!("{root}/target/inner"))?]);
    Ok(())
}

#[test]
fn a_link_to_a_device_is_not_a_second_spelling() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let real = dir.path().canonicalize()?;
    std::os::unix::fs::symlink("/dev/null", real.join(".env"))?;
    let env = lp(&format!("{}/.env", real.to_str().ok_or("a value")?))?;
    assert_eq!(identity().host_spellings(&env)?, vec![env.clone()]);
    assert_eq!(identity().name_spellings(&env)?, vec![env]);
    assert_eq!(identity().host_spellings(&lp("/dev/null")?)?, vec![lp("/dev/null")?]);
    Ok(())
}

#[test]
fn a_set_that_spans_path_segments_matches_the_separator() -> Result<(), Box<dyn std::error::Error>> {
    let entries = [glob("/work/a[b/c]d/**/*.key")?];
    let region = Region::translate(&entries, &identity())?;
    let source = &regexes(&region.filters)[0];
    assert!(matches_source(source, "/work/a/d/x/k.key"));
    assert!(matches_source(source, "/work/abd/k.key"));
    assert!(!matches_source(source, "/work/aed/k.key"));
    Ok(())
}

#[test]
fn a_glob_rewrites_its_base_to_the_host() -> Result<(), Box<dyn std::error::Error>> {
    let mapper = PathMapper::new(vec![(lp("/workspace")?, lp("/host/ws")?)]);
    let entries = [glob("/workspace/**/.env")?];
    let region = Region::translate(&entries, &mapper)?;
    let source = &regexes(&region.filters)[0];
    assert!(matches_source(source, "/host/ws/.env"));
    assert!(matches_source(source, "/host/ws/a/b/.env"));
    assert!(!matches_source(source, "/workspace/.env"));
    Ok(())
}

#[test]
fn a_deny_glob_covers_the_subtree_below_each_match() -> Result<(), Box<dyn std::error::Error>> {
    let entries = [glob("/w/**/secrets")?];
    let region = Region::translate(&entries, &identity())?;
    let source = &regexes(&region.filters)[0];
    assert!(matches_source(source, "/w/a/secrets"));
    assert!(matches_source(source, "/w/a/secrets/key"));
    assert!(!matches_source(source, "/w/a/secretsx"));
    Ok(())
}

#[test]
fn glob_regexes_agree_with_the_policy_matcher() -> Result<(), Box<dyn std::error::Error>> {
    let patterns = [
        "/w/*.txt",
        "/w/**/x",
        "/w/a?c",
        "/w/[a-c]x/**",
        "/w/f[0-9]",
        "/w/**/dir/**/f*",
        "/w/a**",
    ];
    let paths = [
        "/w/a.txt",
        "/w/x",
        "/w/q/x",
        "/w/q/r/x",
        "/w/abc",
        "/w/a/c",
        "/w/bx/y",
        "/w/dx",
        "/w/f7",
        "/w/f",
        "/w/m/dir/f1",
        "/w/dir/n/f",
        "/w/aXb",
        "/w/a.b",
        "/w/axb",
        "/w/ab/cd",
    ];
    for pattern in patterns {
        let glob = GlobPattern::new(pattern)?;
        let matcher = trogon_sandbox_protocol::GlobMatcher::new(&glob);
        let source = glob_regex(&glob)?;
        for path in paths {
            assert_eq!(
                matches_source(&source, path),
                matcher.matches(&lp(path)?),
                "{pattern} vs {path} via {source}"
            );
        }
    }
    Ok(())
}

#[test]
fn a_wide_non_alphanumeric_set_is_refused() -> Result<(), Box<dyn std::error::Error>> {
    assert!(matches!(
        set_regex(&[(b'!' as u16, b'~' as u16)]),
        Err(SbplError::SetTooWide { .. })
    ));
    assert_eq!(
        set_regex(&[(b'a' as u16, b'c' as u16), (b'x' as u16, b'x' as u16)])?,
        "([a-c]|x)"
    );
    assert_eq!(set_regex(&[(b'.' as u16, b'.' as u16)])?, r"(\.)");
    Ok(())
}

#[test]
fn literals_sharing_a_directory_are_grouped() -> Result<(), Box<dyn std::error::Error>> {
    let paths: Vec<LiteralPath> = ["a", "b", "c", "d"]
        .iter()
        .map(|n| lp(&format!("/d/{n}")))
        .collect::<Result<_, _>>()?;
    let filters = subtree_filters(&paths)?;
    assert_eq!(regexes(&filters), vec![r"^/d/(a|b|c|d)(/.*)?$".to_string()]);
    Ok(())
}

#[test]
fn fewer_than_the_group_size_stay_subpaths() -> Result<(), Box<dyn std::error::Error>> {
    let paths: Vec<LiteralPath> = ["a", "b", "c"]
        .iter()
        .map(|n| lp(&format!("/d/{n}")))
        .collect::<Result<_, _>>()?;
    let filters = subtree_filters(&paths)?;
    assert_eq!(filters.len(), 3);
    assert!(regexes(&filters).is_empty());
    Ok(())
}

#[test]
fn grouped_regexes_are_split_under_the_budget() -> Result<(), Box<dyn std::error::Error>> {
    let paths: Vec<LiteralPath> = (0..200)
        .map(|i| lp(&format!("/d/{}{i}", "n".repeat(40))))
        .collect::<Result<_, _>>()?;
    let filters = subtree_filters(&paths)?;
    let sources = regexes(&filters);
    assert!(sources.len() > 1);
    for source in &sources {
        assert!(source.len() <= MAX_REGEX_BYTES, "{}", source.len());
    }
    for path in &paths {
        assert!(sources.iter().any(|s| matches_source(s, path.as_str())), "{path}");
        let below = format!("{path}/deeper");
        assert!(sources.iter().any(|s| matches_source(s, &below)));
    }
    assert!(!sources.iter().any(|s| matches_source(s, "/d/other")));
    Ok(())
}

#[test]
fn same_shaped_children_fold_into_one_regex_over_their_names() -> Result<(), Box<dyn std::error::Error>> {
    let paths: Vec<LiteralPath> = ["a", "b", "c", "d"]
        .iter()
        .flat_map(|n| {
            [
                lp(&format!("/g/worktrees/{n}/commondir")),
                lp(&format!("/g/worktrees/{n}/config.worktree")),
            ]
        })
        .collect::<Result<_, _>>()?;
    let filters = subtree_filters(&paths)?;
    assert_eq!(
        regexes(&filters),
        vec![r"^/g/worktrees/(a|b|c|d)/(commondir|config\.worktree)(/.*)?$".to_string()]
    );
    assert_eq!(filters.len(), 1);
    Ok(())
}

#[test]
fn children_with_different_leaves_are_not_folded_together() -> Result<(), Box<dyn std::error::Error>> {
    let mut paths: Vec<LiteralPath> = ["a", "b", "c", "d"]
        .iter()
        .map(|n| lp(&format!("/g/w/{n}/x")))
        .collect::<Result<_, _>>()?;
    paths.push(lp("/g/w/e/y")?);
    let sources = regexes(&subtree_filters(&paths)?);
    assert!(sources.iter().any(|s| matches_source(s, "/g/w/a/x/deeper")));
    assert!(!sources.iter().any(|s| matches_source(s, "/g/w/a/y")));
    assert!(sources.is_empty() || !sources.iter().any(|s| matches_source(s, "/g/w/e/x")));
    Ok(())
}

#[test]
fn exact_filters_group_siblings_and_match_only_the_path_itself() -> Result<(), Box<dyn std::error::Error>> {
    let paths: Vec<LiteralPath> = ["a", "b", "c", "d"]
        .iter()
        .map(|n| lp(&format!("/p/{n}")))
        .collect::<Result<_, _>>()?;
    let sources = regexes(&exact_filters(&paths)?);
    assert_eq!(sources, vec![r"^/p/(a|b|c|d)$".to_string()]);
    assert!(matches_source(&sources[0], "/p/a"));
    assert!(!matches_source(&sources[0], "/p/a/b"));
    let few = exact_filters(&paths[..3])?;
    assert!(regexes(&few).is_empty());
    assert_eq!(few.len(), 3);
    Ok(())
}

#[test]
fn thousands_of_children_stay_far_below_the_profile_limit() -> Result<(), Box<dyn std::error::Error>> {
    let paths: Vec<LiteralPath> = (0..40_000)
        .map(|i| lp(&format!("/g/worktrees/wt{i:06}/commondir")))
        .collect::<Result<_, _>>()?;
    let filters = subtree_filters(&paths)?;
    let total: usize = regexes(&filters).iter().map(String::len).sum();
    assert!(total < 512 * 1024, "{total}");
    assert!(filters.len() < 2_000);
    Ok(())
}

#[test]
fn a_fragment_that_cannot_fit_one_regex_is_an_error() -> Result<(), Box<dyn std::error::Error>> {
    let fragments = vec!["x".repeat(MAX_REGEX_BYTES)];
    assert!(matches!(
        alternation_regexes("^/", &fragments, SUBTREE_TAIL),
        Err(SbplError::StringTooLong { .. })
    ));
    Ok(())
}

#[test]
fn a_name_too_long_for_the_group_stays_a_subpath() -> Result<(), Box<dyn std::error::Error>> {
    let long = "n".repeat(MAX_REGEX_BYTES);
    let mut paths: Vec<LiteralPath> = (0..5).map(|i| lp(&format!("/d/f{i}"))).collect::<Result<_, _>>()?;
    paths.push(lp(&format!("/d/{long}"))?);
    let filters = subtree_filters(&paths)?;
    let rendered = format!("{filters:?}");
    assert!(rendered.contains(&long), "{rendered}");
    assert!(regexes(&filters).iter().all(|s| s.len() <= MAX_REGEX_BYTES));
    assert!(regexes(&filters).iter().any(|s| matches_source(s, "/d/f3/x")));
    Ok(())
}

#[test]
fn too_few_short_names_all_stay_subpaths() -> Result<(), Box<dyn std::error::Error>> {
    let long = "n".repeat(MAX_REGEX_BYTES);
    let paths = vec![lp("/d/a")?, lp("/d/b")?, lp("/d/c")?, lp(&format!("/d/{long}"))?];
    assert!(regexes(&subtree_filters(&paths)?).is_empty());
    assert!(regexes(&exact_filters(&paths)?).is_empty());
    Ok(())
}

#[test]
fn a_glob_base_below_a_root_is_pinned_but_a_root_is_not() -> Result<(), Box<dyn std::error::Error>> {
    let roots = [lp("/w")?];
    assert_eq!(
        pinned_bases(&[lp("/w/p/conf")?, lp("/w")?, lp("/etc")?], &roots),
        vec![lp("/w/p/conf")?]
    );
    Ok(())
}

#[test]
fn an_allow_glob_matches_exactly_and_a_deny_glob_covers_the_subtree() -> Result<(), Box<dyn std::error::Error>> {
    let entries = [glob("/work/proj*")?];
    let deny = Region::translate(&entries, &identity())?;
    let allow = Region::translate_allows(&entries, &identity())?;
    assert_eq!(regexes(&deny.filters), vec!["^/work/proj[^/]*(/.*)?$"]);
    assert_eq!(regexes(&allow.filters), vec!["^/work/proj[^/]*$"]);
    Ok(())
}

#[test]
fn a_case_variant_of_a_protected_name_is_denied_even_where_a_case_sensitive_regex_misses_it()
-> Result<(), Box<dyn std::error::Error>> {
    let sources = regexes(&mandatory_write_denies(&lp("/r")?, false)?);
    let hit = |path: &str| sources.iter().any(|s| matches_source(s, path));
    for path in [
        "/r/.GIT/hooks/pre-commit",
        "/r/.Git/CONFIG",
        "/r/sub/.GIT/hooks/pre-commit",
        "/r/.GITCONFIG",
    ] {
        assert!(hit(path), "{path}");
    }
    Ok(())
}

#[test]
fn mandatory_denies_match_protected_names_at_any_depth_ignoring_case() -> Result<(), Box<dyn std::error::Error>> {
    let filters = mandatory_write_denies(&lp("/r")?, false)?;
    let sources = regexes(&filters);
    let hit = |path: &str| sources.iter().any(|s| matches_source(s, path));
    for path in [
        "/r/.bashrc",
        "/r/.BASHRC",
        "/r/a/b/.zshrc",
        "/r/.vscode/settings.json",
        "/r/.idea",
        "/r/x/.idea/workspace.xml",
        "/r/.git/hooks",
        "/r/.git/hooks/pre-commit",
        "/r/sub/.git/hooks/pre-commit",
        "/r/sub/.agent/commands/x.md",
        "/r/pkg/.git/config",
        "/r/.git/config.worktree",
        "/r/.agent/commands/x.md",
        "/r/.Agent/Agents/a.md",
    ] {
        assert!(hit(path), "{path}");
    }
    for path in [
        "/r/.bashrc.bak",
        "/r/.git/index",
        "/r/.git/objects/ab/cd",
        "/r/.agent/settings.json",
        "/r/sub/.git/hooks",
        "/r/sub/deep/.vscode",
        "/r/sub/.idea",
        "/r/sub/.agent/commands",
        "/r/src/main.rs",
        "/other/.bashrc",
        "/rx/.bashrc",
    ] {
        assert!(!hit(path), "{path}");
    }
    for source in &sources {
        assert!(source.len() <= MAX_REGEX_BYTES);
    }
    Ok(())
}

#[test]
fn allowing_git_config_frees_config_and_keeps_hooks_and_worktree_config() -> Result<(), Box<dyn std::error::Error>> {
    let sources = regexes(&mandatory_write_denies(&lp("/r")?, true)?);
    let hit = |path: &str| sources.iter().any(|s| matches_source(s, path));
    assert!(!hit("/r/.git/config"));
    assert!(!hit("/r/pkg/.git/config"));
    assert!(hit("/r/.git/hooks/pre-commit"));
    assert!(hit("/r/.git/config.worktree"));
    assert!(hit("/r/.gitconfig"));
    Ok(())
}

#[test]
fn a_root_that_is_the_filesystem_root_has_no_double_slash() -> Result<(), Box<dyn std::error::Error>> {
    let sources = regexes(&mandatory_write_denies(&lp("/")?, false)?);
    assert!(sources.iter().all(|s| s.starts_with("^/")));
    assert!(sources.iter().all(|s| !s.starts_with("^//")));
    Ok(())
}

#[test]
fn a_bracketed_root_stays_literal_in_the_mandatory_regexes() -> Result<(), Box<dyn std::error::Error>> {
    let sources = regexes(&mandatory_write_denies(&lp("/work/a[b/c]d")?, false)?);
    assert!(sources.iter().any(|s| matches_source(s, "/work/a[b/c]d/.bashrc")));
    assert!(!sources.iter().any(|s| matches_source(s, "/work/abd/.bashrc")));
    Ok(())
}

#[test]
fn every_dangerous_path_joined_onto_a_bracketed_or_starred_root_stays_under_that_root()
-> Result<(), Box<dyn std::error::Error>> {
    let names = [
        ".gitconfig",
        ".zshrc",
        ".vscode",
        ".git/config",
        ".git/hooks",
        ".git/hooks/pre-commit",
        ".agent/commands/x",
        "vendor/lib/.git/hooks/pre-commit",
    ];
    for (root, decoys) in [
        ("/tmp/a[b/c]d", vec!["/tmp/abd", "/tmp/acd"]),
        ("/tmp/s*t", vec!["/tmp/sZZt", "/tmp/st"]),
        ("/tmp/q?r", vec!["/tmp/qxr"]),
    ] {
        let sources = regexes(&mandatory_write_denies(&lp(root)?, false)?);
        let covered = |path: &str| sources.iter().any(|s| matches_source(s, path));
        for name in names {
            assert!(covered(&format!("{root}/{name}")), "{root} {name}");
            for decoy in &decoys {
                assert!(!covered(&format!("{decoy}/{name}")), "{decoy} {name}");
            }
        }
        for block in [mandatory_move_blocks(&lp(root)?)?, mandatory_create_blocks(&lp(root)?)?] {
            let Filter::Regex(source) = block else {
                return Err("a regex filter".into());
            };
            assert!(matches_source(source.as_str(), &format!("{root}/.git")));
            for decoy in &decoys {
                assert!(!matches_source(source.as_str(), &format!("{decoy}/.git")));
            }
        }
    }
    Ok(())
}

#[test]
fn move_blocks_name_the_git_and_agent_directories() -> Result<(), Box<dyn std::error::Error>> {
    let Filter::Regex(source) = mandatory_move_blocks(&lp("/r")?)? else {
        return Err("expected a regex".into());
    };
    let source = source.as_str();
    assert!(matches_source(source, "/r/.git"));
    assert!(matches_source(source, "/r/pkg/.Agent"));
    assert!(!matches_source(source, "/r/.github"));
    Ok(())
}

#[test]
fn create_blocks_name_only_the_top_level_directories() -> Result<(), Box<dyn std::error::Error>> {
    let Filter::Regex(source) = mandatory_create_blocks(&lp("/r")?)? else {
        return Err("expected a regex".into());
    };
    let source = source.as_str();
    assert!(matches_source(source, "/r/.git"));
    assert!(matches_source(source, "/r/.GIT"));
    assert!(!matches_source(source, "/r/pkg/.git"));
    assert!(!matches_source(source, "/r/.git/hooks"));
    assert!(!matches_source(source, "/r/.github"));
    Ok(())
}

#[test]
fn ancestors_between_a_root_and_a_protected_path_are_pinned() -> Result<(), Box<dyn std::error::Error>> {
    let pinned = pinned_ancestors(&[lp("/w/a/b/secret")?], &[lp("/w")?]);
    assert_eq!(pinned, vec![lp("/w/a")?, lp("/w/a/b")?]);
    Ok(())
}

#[test]
fn nothing_above_or_outside_the_write_roots_is_pinned() -> Result<(), Box<dyn std::error::Error>> {
    assert!(pinned_ancestors(&[lp("/etc/secret")?], &[lp("/w")?]).is_empty());
    assert!(pinned_ancestors(&[lp("/w/secret")?], &[lp("/w")?]).is_empty());
    Ok(())
}
