use super::*;
use std::collections::BTreeMap;

/// An in-memory tree: path (components joined by `\`, `""` for
/// the root) → entries.
#[derive(Default, Clone)]
struct Mem {
    dirs: BTreeMap<String, Vec<Entry>>,
    unreadable: Vec<String>,
    /// Link path → where it points: a path beneath the profile
    /// (`""` for the profile itself), or `None` for outside it.
    links: BTreeMap<String, Option<String>>,
    /// Links whose own data cannot be decoded.
    broken: Vec<String>,
}

fn entry(name: &str, is_dir: bool) -> Entry {
    Entry {
        name: Name::long(name),
        is_dir,
        skip: None,
        link: false,
    }
}

impl Mem {
    /// Add `path`; a trailing `\` makes it a directory. Parents
    /// are added as directories.
    fn add(&mut self, path: &str) -> &mut Self {
        let is_dir = path.ends_with('\\');
        let comps = split_path(path);
        for i in 0..comps.len() {
            let parent = comps[..i].join("\\");
            let dir = i + 1 < comps.len() || is_dir;
            let list = self.dirs.entry(parent).or_default();
            if !list.iter().any(|e| e.name.long == comps[i]) {
                list.push(entry(&comps[i], dir));
            }
            if dir {
                self.dirs.entry(comps[..=i].join("\\")).or_default();
            }
        }
        self
    }

    fn entry_mut(&mut self, path: &str) -> Result<&mut Entry, Box<dyn std::error::Error>> {
        let comps = split_path(path);
        let parent = comps[..comps.len() - 1].join("\\");
        let name = &comps[comps.len() - 1];
        Ok(self
            .dirs
            .get_mut(&parent)
            .ok_or("missing parent directory")?
            .iter_mut()
            .find(|e| e.name.long == *name)
            .ok_or("missing entry")?)
    }
}

impl Mem {
    /// Make the directory `path` a link to `target`.
    fn link(&mut self, path: &str, target: Option<&str>) -> Result<&mut Self, Box<dyn std::error::Error>> {
        if !self.dirs.contains_key(path) {
            self.add(&format!("{path}\\"));
        }
        let e = self.entry_mut(path)?;
        e.skip = Some("reparse point".into());
        e.link = true;
        self.links.insert(path.to_owned(), target.map(str::to_owned));
        Ok(self)
    }

    /// `path` with every link on the way resolved, as the final
    /// path of a real open is; `None` for outside the profile or
    /// a cycle.
    fn real(&self, path: &str) -> Option<String> {
        let mut cur = path.to_owned();
        for _ in 0..32 {
            let comps = split_path(&cur);
            let hit = (1..=comps.len())
                .map(|n| comps[..n].join("\\"))
                .find(|prefix| self.links.contains_key(prefix));
            let Some(prefix) = hit else {
                return Some(cur);
            };
            let target = self.links[&prefix].clone()?;
            let rest = &cur[prefix.len()..];
            cur = format!("{target}{rest}").trim_start_matches('\\').to_owned();
        }
        None
    }
}

impl Tree for Mem {
    fn resolve_link(&mut self, rel: &[String]) -> LinkTarget {
        if self.broken.contains(&rel.join("\\")) {
            return LinkTarget::Elsewhere("its data cannot be decoded".to_owned());
        }
        match self.real(&rel.join("\\")) {
            Some(real) => LinkTarget::Inside(split_path(&real)),
            None => LinkTarget::Elsewhere("outside the profile".to_owned()),
        }
    }

    fn list(&mut self, rel: &[String]) -> Result<Vec<Entry>, String> {
        let key = rel.join("\\");
        if self.unreadable.contains(&key) {
            return Err("access denied".to_owned());
        }
        self.dirs
            .get(&key)
            .cloned()
            .ok_or_else(|| format!("no directory '{key}'"))
    }
}

fn rules() -> Rules {
    Rules::new(None, None, &[])
}

fn kinds(nodes: &[Node]) -> BTreeMap<String, (Kind, bool)> {
    nodes.iter().map(|n| (n.rel.join("\\"), (n.kind, n.missing))).collect()
}

/// The node planned for `path`.
fn by<'a>(nodes: &'a [Node], path: &str) -> Result<&'a Node, String> {
    nodes
        .iter()
        .find(|n| n.rel.join("\\") == path)
        .ok_or_else(|| format!("no node for '{path}'"))
}

fn names(rel: &[String]) -> Vec<Name> {
    rel.iter().map(|c| Name::long(c)).collect()
}

#[test]
fn a_profile_is_planned_by_the_walk_rule() -> Result<(), Box<dyn std::error::Error>> {
    let mut t = Mem::default();
    t.add(r"Documents\notes.txt")
        .add(r"source\repo\a.rs")
        .add(".npmrc")
        .add(".gitconfig")
        .add(".netrc")
        .add("NTUSER.DAT")
        .add("ntuser.dat.LOG1")
        .add("stray.txt")
        .add(r".ssh\id_ed25519")
        .add(r".cargo\credentials.toml")
        .add(r".cargo\config.toml")
        .add(r".cargo\bin\rg.exe")
        .add(r".config\git\config")
        .add(r".config\git\credentials")
        .add(r".config\gh\hosts.yml")
        .add(r".config\pip\pip.conf")
        .add(r".m2\settings.xml")
        .add(r"AppData\Roaming\npm\tool.cmd")
        .add(r"AppData\Roaming\Microsoft\Credentials\x")
        .add(r"AppData\Roaming\Microsoft\Windows\Start Menu\a.lnk")
        .add(r"AppData\Local\Temp\t.tmp")
        .add(r"AppData\Local\Programs\Python\python.exe")
        .add(r"AppData\Local\SomeApp\EBWebView\Cookies")
        .add(r"AppData\Local\SomeApp\data.bin")
        .add(r"AppData\LocalLow\x");
    let nodes = plan(&mut t, &rules())?;
    let k = kinds(&nodes);
    let is = |p: &str, kind: Kind| {
        assert_eq!(k.get(p).map(|v| v.0), Some(kind), "{p}");
    };
    let absent = |p: &str| assert!(!k.contains_key(p), "{p} is in the plan");
    is("", Kind::Container);
    is("Documents", Kind::Granted);
    is("source", Kind::Granted);
    is(".m2", Kind::Granted);
    absent(r"Documents\notes.txt");
    is(".npmrc", Kind::File);
    is(".gitconfig", Kind::File);
    absent("stray.txt");
    is(".netrc", Kind::Denied);
    is("NTUSER.DAT", Kind::Denied);
    is("ntuser.dat.LOG1", Kind::Denied);
    is(".ssh", Kind::Denied);
    absent(r".ssh\id_ed25519");
    is(".cargo", Kind::Container);
    is(r".cargo\credentials.toml", Kind::Denied);
    is(r".cargo\config.toml", Kind::File);
    is(r".cargo\bin", Kind::Granted);
    is(".config", Kind::Container);
    is(r".config\git", Kind::Container);
    is(r".config\git\config", Kind::File);
    is(r".config\git\credentials", Kind::Denied);
    is(r".config\gh", Kind::Denied);
    is(r".config\pip", Kind::Granted);
    is("AppData", Kind::Container);
    is(r"AppData\Roaming", Kind::Container);
    is(r"AppData\Roaming\npm", Kind::Granted);
    is(r"AppData\Roaming\Microsoft", Kind::Container);
    is(r"AppData\Roaming\Microsoft\Credentials", Kind::Denied);
    is(r"AppData\Roaming\Microsoft\Windows", Kind::Granted);
    is(r"AppData\Local", Kind::Container);
    is(r"AppData\Local\Temp", Kind::Ungranted);
    absent(r"AppData\Local\Temp\t.tmp");
    is(r"AppData\Local\Programs", Kind::Container);
    is(r"AppData\Local\Programs\Python", Kind::Granted);
    is(r"AppData\Local\SomeApp", Kind::Container);
    is(r"AppData\Local\SomeApp\EBWebView", Kind::Denied);
    is(r"AppData\Local\SomeApp\data.bin", Kind::File);
    is(r"AppData\LocalLow", Kind::Granted);
    assert_eq!(k.get(".aws"), Some(&(Kind::Denied, true)));
    assert_eq!(k.get(".config\\gcloud"), Some(&(Kind::Denied, true)));
    assert_eq!(k.get(r"AppData\Local\Google"), Some(&(Kind::Container, true)));
    assert_eq!(
        k.get(r"AppData\Local\Google\Chrome\User Data"),
        Some(&(Kind::Denied, true))
    );
    absent(".kaggle");
    absent(r"AppData\Local\Vivaldi");
    absent(r"AppData\Roaming\Thunderbird");
    absent(".git-credentials");
    absent(r".gradle");
    assert!(!k.keys().any(|p| p.contains('*')));
    absent(r"AppData\Local\trogon-sandbox");
    for (i, n) in nodes.iter().enumerate() {
        if let Some((_, parent)) = n.rel.split_last() {
            let at = nodes.iter().position(|m| m.rel == parent).ok_or("missing value")?;
            assert!(at < i, "{:?} precedes its parent", n.rel);
        }
    }
    Ok(())
}

#[test]
fn the_state_directory_is_excluded_and_left_to_its_owner() -> Result<(), Box<dyn std::error::Error>> {
    let mut t = Mem::default();
    t.add(r"AppData\Local\trogon-sandbox\state.db");
    let nodes = plan(&mut t, &rules())?;
    let n = nodes
        .iter()
        .find(|n| n.rel.join("\\") == r"AppData\Local\trogon-sandbox")
        .ok_or("missing value")?;
    assert_eq!((n.kind, n.stamp), (Kind::Denied, false));
    Ok(())
}

/// An ungranted-only directory gets nothing and gives its
/// children nothing; a protected path inside it is still denied,
/// and the directories on the way to it get nothing either.
#[test]
fn an_ungranted_only_directory_is_left_bare_but_for_protected_paths_inside() -> Result<(), Box<dyn std::error::Error>> {
    let extra = vec![split_path(r"AppData\Local\Temp\app\keys")];
    let rules = Rules::new(None, None, &extra);
    let mut t = Mem::default();
    t.add(r"AppData\Local\Temp\loose.txt")
        .add(r"AppData\Local\Temp\session-1\work\f.txt")
        .add(r"AppData\Local\Temp\app\keys\ca.key")
        .add(r"AppData\Local\Temp\app\other\o.txt")
        .add(r"AppData\Local\Temp\app\a.txt")
        .add(r"AppData\Local\Other\x");
    let nodes = plan(&mut t, &rules)?;
    let k = kinds(&nodes);
    let got: Vec<(&str, Kind)> = k
        .iter()
        .filter(|(p, _)| p.starts_with(r"AppData\Local\Temp"))
        .map(|(p, v)| (p.as_str(), v.0))
        .collect();
    assert_eq!(
        got,
        [
            (r"AppData\Local\Temp", Kind::Ungranted),
            (r"AppData\Local\Temp\app", Kind::Ungranted),
            (r"AppData\Local\Temp\app\keys", Kind::Denied),
        ]
    );
    assert!(!k.contains_key(r"AppData\Local\Temp\session-1"));
    let mut t = Mem::default();
    t.add(r"AppData\Local\x");
    let k = kinds(&plan(&mut t, &rules)?);
    assert!(!k.keys().any(|p| p.starts_with(r"AppData\Local\Temp")));
    Ok(())
}

/// A link where a rule points: the rule follows it to its target.
#[test]
fn a_rule_follows_a_link_to_its_target_inside_the_profile() -> Result<(), Box<dyn std::error::Error>> {
    let mut t = Mem::default();
    t.add(r"dotfiles\.ssh\id_ed25519")
        .add(r"dotfiles\.config\gh\hosts.yml")
        .add(r"dotfiles\.config\git\config")
        .add(r"dotfiles\.config\git\credentials")
        .add(r"dotfiles\.config\nvim\init.lua")
        .add(r"dotfiles\README")
        .add(r"Documents\a.txt");
    t.link(".ssh", Some(r"dotfiles\.ssh"))?
        .link(".config", Some(r"dotfiles\.config"))?;
    let (nodes, rules) = plan_resolved(&mut t, &rules())?;
    let k = kinds(&nodes);
    let is = |p: &str, kind: Kind| assert_eq!(k.get(p).map(|v| v.0), Some(kind), "{p}");
    is(".ssh", Kind::Skipped);
    is(".config", Kind::Skipped);
    is("dotfiles", Kind::Container);
    is(r"dotfiles\.ssh", Kind::Denied);
    is(r"dotfiles\.config", Kind::Container);
    is(r"dotfiles\.config\gh", Kind::Denied);
    is(r"dotfiles\.config\git", Kind::Container);
    is(r"dotfiles\.config\git\credentials", Kind::Denied);
    is(r"dotfiles\.config\git\config", Kind::File);
    is(r"dotfiles\.config\nvim", Kind::Granted);
    is(r"dotfiles\README", Kind::File);
    is("Documents", Kind::Granted);
    assert!(!k.contains_key(r"dotfiles\.ssh\id_ed25519"));
    let spelled = rules.spelled();
    for want in [r"dotfiles\.ssh", r"dotfiles\.config\gh", r"dotfiles\.config\gcloud"] {
        assert!(spelled.iter().any(|s| s == want), "{want} not in {spelled:?}");
    }
    assert!(rules.excludes(&names(&split_path(r"dotfiles\.ssh\new-key"))));
    assert!(rules.could_exclude_beneath(&names(&split_path("dotfiles"))));
    let mut plain = Mem::default();
    plain.add(r"dotfiles\.ssh\id_ed25519").add(r"dotfiles\README");
    let k = kinds(&plan(&mut plain, &super::tests::rules())?);
    assert_eq!(k.get("dotfiles").map(|v| v.0), Some(Kind::Granted));
    Ok(())
}

/// A link's destination is worked out from the link's own data,
/// and is a destination only on a drive letter.
#[test]
fn a_link_is_followed_only_to_a_drive_of_this_machine() -> Result<(), Box<dyn std::error::Error>> {
    let link = r"\\?\C:\Users\me\.ssh";
    let rest = ["keys".to_owned(), "id".to_owned()];
    let j = |t: &str| LinkData::Junction(t.to_owned());
    let sym = |t: &str, relative: bool| LinkData::Symlink {
        target: t.to_owned(),
        relative,
    };
    let to = |d: &LinkData| link_destination(link, d, &rest);
    assert_eq!(
        to(&j(r"\??\C:\Users\me\dotfiles\.ssh"))?,
        r"\\?\C:\Users\me\dotfiles\.ssh\keys\id"
    );
    assert_eq!(to(&j(r"\??\d:\elsewhere\"))?, r"\\?\D:\elsewhere\keys\id");
    assert_eq!(to(&sym(r"\??\C:\x", false))?, r"\\?\C:\x\keys\id");
    assert_eq!(
        to(&sym(r"dotfiles\.ssh", true))?,
        r"\\?\C:\Users\me\dotfiles\.ssh\keys\id"
    );
    assert_eq!(to(&sym(r"..\shared\.\ssh", true))?, r"\\?\C:\Users\shared\ssh\keys\id");
    assert_eq!(to(&sym(r"\data\ssh", true))?, r"\\?\C:\data\ssh\keys\id");
    assert_eq!(link_destination(link, &sym("..", true), &[])?, r"\\?\C:\Users");
    for d in [
        j(r"\??\UNC\server\share\x"),
        sym(r"\??\UNC\server\share\x", false),
        sym(r"\\server\share\x", false),
        sym(r"\\server\share\x", true),
        sym(r"\\?\UNC\server\share", false),
        j(r"\??\Volume{01234567-89ab-cdef-0123-456789abcdef}\x"),
        sym(r"\Device\Mup\server\share", false),
        sym(r"..\..\..\..\x", true),
        LinkData::Other(0x9000_001A),
    ] {
        assert!(to(&d).is_err(), "{d:?} was followed to {:?}", to(&d));
    }
    assert!(link_destination(r"\\?\UNC\s\h\l", &sym("x", true), &[]).is_err());

    let wide = |s: &str| -> Vec<u8> { s.encode_utf16().flat_map(u16::to_le_bytes).collect() };
    let buffer = |tag: u32, flags: Option<u32>, subst: &str, print: &str| {
        let (s, p) = (wide(subst), wide(print));
        let mut b = Vec::new();
        b.extend(tag.to_le_bytes());
        b.extend(0u16.to_le_bytes());
        b.extend(0u16.to_le_bytes());
        b.extend(0u16.to_le_bytes());
        b.extend((s.len() as u16).to_le_bytes());
        b.extend((s.len() as u16).to_le_bytes());
        b.extend((p.len() as u16).to_le_bytes());
        if let Some(f) = flags {
            b.extend(f.to_le_bytes());
        }
        b.extend(&s);
        b.extend(&p);
        b
    };
    assert_eq!(
        parse_reparse(&buffer(0xA000_0003, None, r"\??\C:\t", r"C:\t")),
        Ok(j(r"\??\C:\t"))
    );
    assert_eq!(
        parse_reparse(&buffer(0xA000_000C, Some(1), r"..\t", r"..\t")),
        Ok(sym(r"..\t", true))
    );
    assert_eq!(
        parse_reparse(&buffer(0xA000_000C, Some(0), r"\??\UNC\s\h", "")),
        Ok(sym(r"\??\UNC\s\h", false))
    );
    assert_eq!(
        parse_reparse(&buffer(0x9000_001A, None, "", "")),
        Ok(LinkData::Other(0x9000_001A))
    );
    assert!(parse_reparse(&[0x03, 0, 0, 0xA0, 0, 0]).is_err());
    let mut cut = buffer(0xA000_0003, None, r"\??\C:\t", "");
    cut.truncate(cut.len() - 4);
    assert!(parse_reparse(&cut).is_err());
    Ok(())
}

/// A link whose target is not there yet, and one that cannot be
/// read.
#[test]
fn a_missing_link_target_is_still_protected_and_an_undecodable_link_is_skipped()
-> Result<(), Box<dyn std::error::Error>> {
    let mut t = Mem::default();
    t.add(r"dotfiles\README").add(r"Documents\a.txt");
    t.link(".ssh", Some(r"dotfiles\.ssh"))?;
    let (nodes, rules) = plan_resolved(&mut t, &rules())?;
    let k = kinds(&nodes);
    assert_eq!(k.get("dotfiles").map(|v| v.0), Some(Kind::Container));
    assert_eq!(k.get(r"dotfiles\.ssh"), Some(&(Kind::Denied, true)));
    assert_eq!(k.get(r"dotfiles\README").map(|v| v.0), Some(Kind::File));
    assert!(rules.excludes(&names(&split_path(r"dotfiles\.ssh\id_new"))));
    let mut t = Mem::default();
    t.add(r"Documents\a.txt");
    t.link(".ssh", Some(r"dotfiles\.ssh"))?;
    let nodes = plan(&mut t, &super::tests::rules())?;
    let k = kinds(&nodes);
    assert_eq!(k.get("dotfiles"), Some(&(Kind::Container, true)));
    assert_eq!(k.get(r"dotfiles\.ssh"), Some(&(Kind::Denied, true)));
    let mut t = Mem::default();
    t.add(r"dotfiles\README");
    t.link(".config", Some(r"dotfiles\.config"))?;
    let (nodes, rules) = plan_resolved(&mut t, &super::tests::rules())?;
    assert!(!kinds(&nodes).contains_key(r"dotfiles\.config\hub"));
    assert!(rules.excludes(&names(&split_path(r"dotfiles\.config\hub"))));

    let mut t = Mem::default();
    t.add(r"dotfiles\.ssh\id").add(r"Documents\a.txt");
    t.link(".ssh", Some(r"dotfiles\.ssh"))?;
    t.broken.push(".ssh".to_owned());
    let k = kinds(&plan(&mut t, &super::tests::rules())?);
    assert_eq!(k.get(".ssh").map(|v| v.0), Some(Kind::Skipped));
    assert_eq!(k.get("").map(|v| v.0), Some(Kind::Container));
    assert_eq!(k.get("Documents").map(|v| v.0), Some(Kind::Granted));
    Ok(())
}

/// A rule carried over to a link's target keeps the source rule's
/// way of being applied.
#[test]
fn a_carried_over_rule_keeps_the_flags_of_its_source() -> Result<(), Box<dyn std::error::Error>> {
    let mut t = Mem::default();
    t.add(r"data\Local\trogon-sandbox\state.db")
        .add(r"data\Local\Microsoft\Credentials\blob")
        .add(r"data\Local\Programs\tool.exe")
        .add(r"AppData\Roaming\npm\x.cmd");
    t.link(r"AppData\Local", Some(r"data\Local"))?;
    let original = rules();
    let (nodes, rules) = plan_resolved(&mut t, &original)?;
    let state = by(&nodes, r"data\Local\trogon-sandbox")?;
    assert_eq!(state.kind, Kind::Denied);
    assert!(!state.stamp, "the state directory is left to its owner");
    let creds = by(&nodes, r"data\Local\Microsoft\Credentials")?;
    assert!(creds.kind == Kind::Denied && creds.stamp);
    assert!(rules.ungranted(&names(&split_path(r"data\Local\Temp"))));
    assert!(!rules.excludes(&names(&split_path(r"data\Local\Temp"))));
    assert!(rules.rules.len() > original.rules.len());
    for d in &rules.rules[original.rules.len()..] {
        assert!(
            original.rules.iter().any(|r| {
                r.stamp == d.stamp
                    && r.placeholder == d.placeholder
                    && r.ungranted == d.ungranted
                    && (0..r.pats.len()).any(|j| d.pats.ends_with(&r.pats[j..]))
            }),
            "{:?} has the flags of no rule it could come from",
            d.spelled
        );
    }
    Ok(())
}

#[test]
fn links_in_chains_upwards_outwards_and_in_circles() -> Result<(), Box<dyn std::error::Error>> {
    let mut t = Mem::default();
    t.add(r"a\cfg\other\x").add(r"b\gh\hosts.yml").add(r"b\keep.txt");
    t.link(".config", Some(r"a\cfg"))?.link(r"a\cfg\gh", Some(r"b\gh"))?;
    let (nodes, rules) = plan_resolved(&mut t, &rules())?;
    let k = kinds(&nodes);
    assert_eq!(k.get(r"b\gh").map(|v| v.0), Some(Kind::Denied));
    assert_eq!(k.get("b").map(|v| v.0), Some(Kind::Container));
    assert_eq!(k.get(r"b\keep.txt").map(|v| v.0), Some(Kind::File));
    assert_eq!(k.get(r"a\cfg").map(|v| v.0), Some(Kind::Container));
    assert_eq!(k.get(r"a\cfg\gh").map(|v| v.0), Some(Kind::Skipped));
    assert!(rules.spelled().iter().any(|s| s == r"b\gh"));

    let mut t = Mem::default();
    t.add(r".config\git\config");
    t.link(r".config\gh", Some(".config"))?;
    let k = kinds(&plan(&mut t, &super::tests::rules())?);
    assert_eq!(k.get(".config").map(|v| v.0), Some(Kind::Denied));
    assert!(!k.contains_key(r".config\git"));

    let mut t = Mem::default();
    t.add(r"Documents\a.txt");
    t.link(".ssh", None)?;
    let (nodes, rules) = plan_resolved(&mut t, &super::tests::rules())?;
    let k = kinds(&nodes);
    assert_eq!(k.get(".ssh").map(|v| v.0), Some(Kind::Skipped));
    assert_eq!(k.get("Documents").map(|v| v.0), Some(Kind::Granted));
    assert_eq!(rules.spelled(), super::tests::rules().spelled());

    let mut t = Mem::default();
    t.link(".ssh", Some(".aws"))?.link(".aws", Some(".ssh"))?;
    let k = kinds(&plan(&mut t, &super::tests::rules())?);
    assert_eq!(k.get(".ssh").map(|v| v.0), Some(Kind::Skipped));
    assert_eq!(k.get(".aws").map(|v| v.0), Some(Kind::Skipped));

    let mut t = Mem::default();
    t.add(r"work\x");
    t.link("shortcut", Some("work"))?;
    let (nodes, rules) = plan_resolved(&mut t, &super::tests::rules())?;
    assert_eq!(kinds(&nodes).get("work").map(|v| v.0), Some(Kind::Granted));
    assert_eq!(rules.spelled(), super::tests::rules().spelled());

    let mut t = Mem::default();
    t.link(".ssh", Some(""))?;
    assert!(plan(&mut t, &super::tests::rules()).is_err());

    let hops = MAX_LINK_ROUNDS + 3;
    let long: Vec<String> = (1..=hops + 1).map(|i| format!("x{i}")).collect();
    let chain = |links: usize| -> Result<_, Box<dyn std::error::Error>> {
        let mut t = Mem::default();
        for i in 1..=links {
            let at = if i == 1 {
                "x1".to_owned()
            } else {
                format!(r"h{}\x{i}", i - 1)
            };
            t.add(&format!(r"h{i}\keep.txt"));
            t.link(&at, Some(&format!("h{i}")))?;
        }
        Ok(t)
    };
    let with_long = Rules::new(None, None, std::slice::from_ref(&long));
    assert!(plan(&mut chain(hops)?, &with_long).is_err());
    let (nodes, rules) = plan_resolved(&mut chain(3)?, &with_long)?;
    assert_eq!(kinds(&nodes).get("h3").map(|v| v.0), Some(Kind::Container));
    let rest = long[3..].join("\\");
    assert!(rules.spelled().contains(&format!(r"h3\{rest}")));
    Ok(())
}

#[test]
fn reparse_points_and_unreadable_directories_are_skipped_at_every_level() -> Result<(), Box<dyn std::error::Error>> {
    let mut t = Mem::default();
    t.add(r"link\")
        .add(r"OneDrive\")
        .add(r"AppData\Local\junction\")
        .add(r"AppData\Local\app\link.lnk")
        .add(r".config\x\")
        .add(r".ssh\");
    t.entry_mut("link")?.skip = Some("reparse point".into());
    t.entry_mut("OneDrive")?.skip = Some("cloud placeholder".into());
    t.entry_mut(r"AppData\Local\junction")?.skip = Some("reparse point".into());
    t.entry_mut(r"AppData\Local\app\link.lnk")?.skip = Some("reparse point".into());
    t.entry_mut(".ssh")?.skip = Some("reparse point".into());
    t.unreadable.push(".config".into());
    let nodes = plan(&mut t, &rules())?;
    let k = kinds(&nodes);
    for p in [
        "link",
        "OneDrive",
        r"AppData\Local\junction",
        r"AppData\Local\app\link.lnk",
        ".ssh",
        ".config",
    ] {
        assert_eq!(k.get(p).map(|v| v.0), Some(Kind::Skipped), "{p}");
    }
    assert!(!k.keys().any(|p| p.starts_with(r".config\")));
    let skipped: Vec<&Node> = nodes.iter().filter(|n| n.kind == Kind::Skipped).collect();
    assert!(skipped.iter().all(|n| n.note.is_some()));
    let mut bad = Mem::default();
    bad.unreadable.push(String::new());
    assert!(plan(&mut bad, &rules()).is_err());
    Ok(())
}

#[test]
fn a_name_cannot_be_respelled_past_an_exclusion() -> Result<(), Box<dyn std::error::Error>> {
    let r = rules();
    let ex = |p: &str| r.excludes(&names(&split_path(p)));
    for p in [
        ".ssh",
        ".SSH",
        ".ssh.",
        ".ssh ",
        ".ssh. .",
        ".ssh::$INDEX_ALLOCATION",
        ".ssh:stream",
        r".ssh\id_rsa",
        r".Ssh.\ID_RSA",
        ".netrc::$DATA",
        ".NETRC",
        "NTUSER.DAT",
        "ntuser.dat.LOG2",
        "NTUSER.DAT{guid}.TM.blf",
        r"appdata\roaming\microsoft\CREDENTIALS",
        r"AppData\Local\Anything\EBWebView\Default",
        r"AppData\Local\Microsoft\Olk\EBWebView",
        r"AppData\Local\Microsoft\Windows\UsrClass.dat.LOG1",
        r".cargo\Credentials.TOML",
    ] {
        assert!(ex(p), "{p} is not excluded");
    }
    assert!(!ex(r"AppData\Local\Temp"));
    assert!(!ex(r"AppData\Local\Temp\x\y"));
    for p in [r"AppData\Local\Temp", r"appdata\LOCAL\temp.\x\y"] {
        assert!(r.ungranted(&names(&split_path(p))), "{p}");
    }
    assert!(!r.ungranted(&names(&split_path(r"AppData\Local\Temp2"))));
    assert!(!r.ungranted(&names(&split_path(".ssh"))));
    for p in [
        ".sshx",
        "ssh",
        ".npmrc",
        ".gitconfig",
        r".cargo\config.toml",
        r"Documents\.ssh",
        r"AppData\Local\EBWebView",
        r"AppData\Roaming\npm",
        "..",
        ". .",
    ] {
        assert!(!ex(p), "{p} is excluded");
    }
    let short = |long: &str, short: &str| Name {
        long: long.to_owned(),
        short: Some(short.to_owned()),
    };
    let r = Rules::new(
        None,
        None,
        &[vec!["MYSECR~1".to_owned()], vec!["Long Secret Dir".to_owned()]],
    );
    assert!(r.excludes(&[short("My Secrets", "MYSECR~1")]));
    assert!(r.excludes(&[short("Long Secret Dir", "LONGSE~1")]));
    assert!(r.excludes(&[Name::long("LONG SECRET DIR. ")]));
    assert!(!r.excludes(&[short("My Secrets 2", "MYSECR~2")]));
    let mut t = Mem::default();
    t.add(r"My Secrets\k");
    t.entry_mut("My Secrets")?.name.short = Some("MYSECR~1".into());
    let k = kinds(&plan(&mut t, &r)?);
    assert_eq!(k.get("My Secrets").map(|v| v.0), Some(Kind::Denied));
    Ok(())
}

#[test]
fn exclusion_paths_from_configuration() -> Result<(), Box<dyn std::error::Error>> {
    let profile = r"\\?\C:\Users\me";
    let ok = |p: &str| rel_components(profile, p).map(|c| c.join("\\"));
    assert_eq!(ok(r"C:\Users\me\.myapp")?, ".myapp");
    assert_eq!(ok(r"c:/users/ME/.myapp/tokens")?, r".myapp\tokens");
    assert_eq!(ok(r"\\?\C:\Users\me\AppData\Roaming\MyApp")?, r"AppData\Roaming\MyApp");
    assert_eq!(ok(r".myapp\")?, ".myapp");
    assert_eq!(ok(r"C:\Users\me")?, "");
    for bad in [
        r"C:\Users\other\.myapp",
        r"C:\Users\me\..\other",
        r"D:\x",
        r".myapp\*",
        r".\x",
        r"C:\Users\me2",
    ] {
        assert!(rel_components(profile, bad).is_err(), "{bad}");
    }
    let r = Rules::new(None, None, &[vec![".myapp".to_owned()], vec![]]);
    assert!(r.excludes(&names(&split_path(r".MyApp\token"))));
    assert!(!r.excludes(&names(&split_path("Documents"))));
    assert!(r.excludes(&names(&split_path(".ssh"))));
    let spelled = r.spelled();
    for want in [
        ".myapp",
        ".ssh",
        r"AppData\Roaming\Microsoft\Credentials",
        r"AppData\Local\*\EBWebView",
        "NTUSER.DAT*",
        r"AppData\Local\trogon-sandbox",
    ] {
        assert_eq!(spelled.iter().filter(|s| s.as_str() == want).count(), 1, "{want}");
    }
    assert_eq!(r.spelled_ungranted(), [r"AppData\Local\Temp"]);
    assert!(!spelled.contains(&r"AppData\Local\Temp".to_owned()));
    Ok(())
}

/// Both spellings of an application-data location are covered:
/// the conventional one and a known folder that sits elsewhere in
/// the profile.
#[test]
fn application_data_entries_follow_the_known_folders() {
    let moved = ["Data".to_owned(), "Roam".to_owned()];
    let r = Rules::new(Some(&moved), None, &[]);
    let ex = |p: &str| r.excludes(&names(&split_path(p)));
    assert!(ex(r"Data\Roam\Microsoft\Credentials"));
    assert!(ex(r"Data\Roam\gcloud"));
    assert!(ex(r"AppData\Roaming\Microsoft\Credentials"));
    assert!(r.ungranted(&names(&split_path(r"AppData\Local\Temp"))));
    assert!(!ex(r"Data\Roam\npm"));
    let r = rules();
    for (base, path) in [
        (Roaming, "gcloud"),
        (Roaming, "GitHub CLI"),
        (Roaming, "com.vercel.cli"),
        (Roaming, "netlify"),
        (Local, "heroku"),
        (Local, ".IdentityService"),
        (Local, "MicrosoftCredentialProvider"),
        (Local, r"Microsoft\IdentityCache"),
        (Local, r"Microsoft\TokenBroker"),
        (Local, r"Microsoft\OneAuth"),
        (Local, r"Microsoft\Windows\INetCookies"),
        (Local, r"Microsoft\Windows\UsrClass.dat"),
        (Profile, r".config\gcloud"),
        (Profile, r".config\gh"),
        (Local, r"github-copilot\hosts.json"),
        (Profile, r".config\github-copilot\apps.json"),
        (Profile, r".gem\credentials"),
        (Profile, r".pulumi\credentials.json"),
        (Roaming, r"postgresql\pgpass.conf"),
        (Profile, ".pgpass"),
        (Roaming, r"Composer\auth.json"),
        (Profile, r".composer\auth.json"),
        (Profile, r".config\doctl\config.yaml"),
        (Profile, r".config\hub"),
        (Profile, r".config\op\config"),
        (Profile, r".kaggle\kaggle.json"),
        (Profile, ".databrickscfg"),
        (Profile, r".snowsql\config"),
        (Profile, ".boto"),
        (Profile, ".s3cfg"),
        (Profile, r".oci\config"),
        (Profile, r".fly\config.yml"),
        (Local, r"Google\Chrome Beta\User Data\Default\Cookies"),
        (Local, r"Chromium\User Data"),
        (Local, r"Vivaldi\User Data"),
        (Local, r"Yandex\YandexBrowser\User Data"),
        (Roaming, r"Opera Software\Opera GX Stable"),
        (Roaming, r"Thunderbird\Profiles"),
    ] {
        let prefix = match base {
            Roaming => r"AppData\Roaming\",
            Local => r"AppData\Local\",
            Profile => "",
        };
        let full = format!("{prefix}{path}");
        assert!(r.excludes(&names(&split_path(&full))), "{full}");
    }
    assert!(
        BUILT_IN
            .iter()
            .all(|e| !e.path.is_empty() && !e.path.starts_with('\\') && !e.path.contains('/') && !e.path.contains(':'))
    );
    for tool in [r".pulumi\bin\pulumi.exe", r".fly\bin\flyctl.exe", r".gem\specs"] {
        assert!(!r.excludes(&names(&split_path(tool))), "{tool}");
    }
    for (base, path, _) in READABLE_CONFIG {
        let prefix = match base {
            Roaming => r"AppData\Roaming\",
            Local => r"AppData\Local\",
            Profile => "",
        };
        let full = format!("{prefix}{path}");
        assert!(!r.excludes(&names(&split_path(&full))), "{full}");
    }
}

/// A small deterministic generator, so the property below needs
/// no dependency.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

const POOL: &[&str] = &[
    ".ssh",
    ".SSH.",
    ".aws",
    ".config",
    "gcloud",
    "gh",
    "git",
    "credentials",
    "config",
    ".cargo",
    "credentials.toml",
    "AppData",
    "Roaming",
    "Local",
    "Temp",
    "Microsoft",
    "Credentials",
    "Windows",
    "UsrClass.dat",
    "EBWebView",
    "Google",
    "Chrome",
    "User Data",
    "Packages",
    ".npmrc",
    ".netrc",
    "NTUSER.DAT",
    "Documents",
    "src",
    "a",
    "b",
    "c.txt",
    "d.exe",
    ".myapp",
    "tokens",
    "keys",
];

fn random_tree(rng: &mut Rng) -> Result<Mem, Box<dyn std::error::Error>> {
    let mut t = Mem::default();
    t.dirs.entry(String::new()).or_default();
    for _ in 0..rng.below(40) + 5 {
        let depth = rng.below(5) + 1;
        let mut p: Vec<&str> = (0..depth).map(|_| POOL[rng.below(POOL.len())]).collect();
        match rng.below(4) {
            0 => p.insert(0, "AppData"),
            1 => {
                p.insert(0, "Local");
                p.insert(0, "AppData");
            }
            _ => {}
        }
        let mut path = p.join("\\");
        if rng.below(2) == 0 {
            path.push('\\');
        }
        let comps = split_path(&path);
        let clash = (0..comps.len()).any(|i| {
            let parent = comps[..i].join("\\");
            let dir = i + 1 < comps.len() || path.ends_with('\\');
            t.dirs
                .get(&parent)
                .is_some_and(|l| l.iter().any(|e| e.name.long == comps[i] && e.is_dir != dir))
        });
        if !clash {
            t.add(&path);
        }
    }
    let keys: Vec<String> = t.dirs.keys().filter(|k| !k.is_empty()).cloned().collect();
    for k in &keys {
        match rng.below(10) {
            0 => {
                let target = match rng.below(4) {
                    0 => None,
                    _ => Some(keys[rng.below(keys.len())].clone()),
                };
                t.link(k, target.as_deref())?;
            }
            1 => t.unreadable.push(k.clone()),
            _ => {}
        }
    }
    Ok(t)
}

/// The invariant, over generated trees: no excluded path - on
/// disk now or created later - has an ancestor carrying an
/// inheriting allow, and nothing excluded is granted itself.
#[test]
fn no_exclusion_ever_lies_beneath_an_inheriting_allow() -> Result<(), Box<dyn std::error::Error>> {
    let extra = vec![
        vec![".myapp".to_owned(), "tokens".to_owned()],
        vec!["Documents".to_owned(), "a".to_owned()],
        split_path(r"AppData\Local\Temp\keys"),
    ];
    let rules = Rules::new(None, None, &extra);
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let original = rules;
    for round in 0..600 {
        let mut t = random_tree(&mut rng)?;
        let (nodes, rules) = plan_resolved(&mut t, &original)?;
        for (n, d) in rules.rules.iter().enumerate().skip(original.rules.len()) {
            assert!(
                rules.rules[..n].iter().any(|r| {
                    r.stamp == d.stamp
                        && r.placeholder == d.placeholder
                        && r.ungranted == d.ungranted
                        && (0..=r.pats.len()).any(|j| d.pats.ends_with(&r.pats[j..]))
                }),
                "round {round}: {:?} has the flags of no rule it could come from",
                d.spelled
            );
        }
        let by_rel: BTreeMap<String, &Node> = nodes.iter().map(|n| (n.rel.join("\\"), n)).collect();
        assert_eq!(by_rel.len(), nodes.len(), "round {round}: a path twice");
        for n in &nodes {
            let rel = names(&n.rel);
            let path = n.rel.join("\\");
            match n.kind {
                Kind::Granted => {
                    assert!(!rules.excludes(&rel), "round {round}: {path}");
                    assert!(
                        !rules.could_exclude_beneath(&rel),
                        "round {round}: an exclusion can lie beneath granted {path}"
                    );
                }
                Kind::File => assert!(!rules.excludes(&rel), "round {round}: {path}"),
                Kind::Denied => assert!(rules.excludes(&rel), "round {round}: {path}"),
                Kind::Ungranted => {
                    assert!(rules.ungranted(&rel), "round {round}: {path}");
                    assert!(!rules.excludes(&rel), "round {round}: {path}");
                }
                Kind::Container | Kind::Skipped => {}
            }
            if rules.ungranted(&rel) {
                assert!(
                    matches!(n.kind, Kind::Ungranted | Kind::Denied | Kind::Skipped),
                    "round {round}: {path} is {:?} inside an ungranted-only directory",
                    n.kind
                );
            }
            for up in 1..n.rel.len() {
                let anc = n.rel[..up].join("\\");
                let a = by_rel
                    .get(&anc)
                    .ok_or_else(|| format!("round {round}: {path} has no planned parent"))?;
                assert!(
                    matches!(a.kind, Kind::Container | Kind::Ungranted),
                    "round {round}: {path} under {anc} ({:?})",
                    a.kind
                );
            }
            if !n.rel.is_empty() {
                assert_eq!(by_rel[""].kind, Kind::Container);
            }
        }
        for (link, _) in t.links.iter().filter(|(l, _)| {
            let parent = split_path(l);
            by_rel.get(l.as_str()).is_some_and(|n| n.kind == Kind::Skipped)
                && !original.ungranted(&names(&parent[..parent.len() - 1]))
        }) {
            let Some(target) = t.real(link) else { continue };
            for n in &nodes {
                let real = n.rel.join("\\");
                let rest = if real == target {
                    ""
                } else if let Some(r) = real.strip_prefix(&format!("{target}\\")) {
                    r
                } else if target.is_empty() {
                    real.as_str()
                } else {
                    continue;
                };
                let alias = names(&split_path(&format!("{link}\\{rest}")));
                if original.excludes(&alias) {
                    assert!(
                        matches!(n.kind, Kind::Denied | Kind::Skipped),
                        "round {round}: {real} is {:?} though excluded as {link}\\{rest}",
                        n.kind
                    );
                }
                if n.kind == Kind::Granted {
                    assert!(
                        !original.could_exclude_beneath(&alias),
                        "round {round}: granted {real} is {link}\\{rest}, beneath which \
                         an exclusion can lie"
                    );
                }
                if original.ungranted(&alias) {
                    assert!(
                        matches!(n.kind, Kind::Ungranted | Kind::Denied | Kind::Skipped),
                        "round {round}: {real} is {:?} though ungranted as {link}\\{rest}",
                        n.kind
                    );
                }
            }
        }
        for (dir, entries) in &t.dirs {
            let parent = split_path(dir);
            let walked = (0..=parent.len()).all(|up| {
                by_rel
                    .get(&parent[..up].join("\\"))
                    .is_some_and(|n| matches!(n.kind, Kind::Container | Kind::Ungranted) && !n.missing)
            });
            if !walked {
                continue;
            }
            for e in entries {
                let mut rel = parent.clone();
                rel.push(e.name.long.clone());
                if rules.excludes(&names(&rel)) {
                    let k = by_rel[&rel.join("\\")].kind;
                    assert!(
                        matches!(k, Kind::Denied | Kind::Skipped),
                        "round {round}: excluded {} planned as {k:?}",
                        rel.join("\\")
                    );
                }
            }
        }
    }
    Ok(())
}

/// Deleting an excluded item, or creating one that was not
/// there, changes no directory from container to granted.
#[test]
fn the_containers_do_not_depend_on_what_exists() -> Result<(), Box<dyn std::error::Error>> {
    let rules = rules();
    let mut with = Mem::default();
    with.add(r".config\gcloud\credentials.db")
        .add(r".config\other\x")
        .add(r".cargo\credentials.toml")
        .add(r"AppData\Roaming\Microsoft\Credentials\c")
        .add(r"AppData\Roaming\Microsoft\Other\o");
    let mut without = Mem::default();
    without
        .add(r".config\other\x")
        .add(r".cargo\")
        .add(r"AppData\Roaming\Microsoft\Other\o");
    let a = kinds(&plan(&mut with, &rules)?);
    let b = kinds(&plan(&mut without, &rules)?);
    for p in [
        ".config",
        ".cargo",
        "AppData",
        r"AppData\Roaming",
        r"AppData\Roaming\Microsoft",
    ] {
        assert_eq!(a[p].0, Kind::Container, "{p}");
        assert_eq!(b[p].0, Kind::Container, "{p}");
    }
    Ok(())
}

#[test]
fn configuration_pointers_name_only_readable_files() {
    let profile = r"C:\Users\me";
    let roaming = r"C:\Users\me\AppData\Roaming";
    let all = |_: &str| true;
    let none = |_: &str| false;
    let unset = |_: &str| None;
    let got = env_pointers(profile, Some(roaming), &all, &none, &unset);
    assert_eq!(
        got,
        vec![
            ("NPM_CONFIG_USERCONFIG", r"C:\Users\me\.npmrc".to_owned()),
            ("GIT_CONFIG_GLOBAL", r"C:\Users\me\.gitconfig".to_owned()),
            ("PIP_CONFIG_FILE", r"C:\Users\me\AppData\Roaming\pip\pip.ini".to_owned()),
            (
                "MAVEN_ARGS",
                "-s \"C:\\Users\\me\\.m2\\settings.xml\" \
                 \"-Dsettings.security=C:\\Users\\me\\.m2\\settings-security.xml\""
                    .to_owned()
            ),
        ]
    );
    assert!(env_pointers(profile, Some(roaming), &none, &none, &unset).is_empty());
    let only = |want: &'static [&'static str]| move |p: &str| want.iter().any(|w| p.ends_with(w));
    let got = env_pointers(
        profile,
        Some(roaming),
        &only(&[r".config\git\config", r"me\pip\pip.ini", "settings.xml"]),
        &none,
        &unset,
    );
    assert_eq!(
        got,
        vec![
            ("GIT_CONFIG_GLOBAL", r"C:\Users\me\.config\git\config".to_owned()),
            ("PIP_CONFIG_FILE", r"C:\Users\me\pip\pip.ini".to_owned()),
            ("MAVEN_ARGS", "-s \"C:\\Users\\me\\.m2\\settings.xml\"".to_owned()),
        ]
    );
    let got = env_pointers(profile, None, &only(&["pip.conf"]), &none, &unset);
    assert_eq!(
        got,
        vec![("PIP_CONFIG_FILE", r"C:\Users\me\.config\pip\pip.conf".to_owned())]
    );
    for (var, _) in env_pointers(profile, Some(roaming), &all, &all, &unset) {
        assert!(
            ![
                "USERPROFILE",
                "HOME",
                "APPDATA",
                "LOCALAPPDATA",
                "TEMP",
                "TMP",
                "GRADLE_USER_HOME",
            ]
            .contains(&var)
        );
    }
}

/// `RUSTUP_HOME` and `CARGO_HOME` are named where the grant reaches them, at
/// the place the real user's environment says, else the default one.
#[test]
fn the_rust_homes_are_named_where_the_grant_reaches_them() {
    let profile = r"C:\Users\me";
    let none = |_: &str| false;
    let unset = |_: &str| None;
    let rust = |dirs: &dyn Fn(&str) -> bool, env: &dyn Fn(&str) -> Option<String>| {
        env_pointers(profile, None, &none, dirs, env)
    };
    assert_eq!(
        rust(&|_| true, &unset),
        vec![
            ("RUSTUP_HOME", r"C:\Users\me\.rustup".to_owned()),
            ("CARGO_HOME", r"C:\Users\me\.cargo".to_owned()),
        ]
    );
    assert!(rust(&none, &unset).is_empty());
    assert_eq!(
        rust(&|p| p.ends_with(".cargo"), &unset),
        vec![("CARGO_HOME", r"C:\Users\me\.cargo".to_owned())]
    );
    let own = |var: &str| match var {
        "RUSTUP_HOME" => Some(r"C:\Users\me\tools\rustup\".to_owned()),
        "CARGO_HOME" => Some(r"D:\cargo".to_owned()),
        _ => None,
    };
    assert_eq!(
        rust(&|p| p.starts_with(r"C:\Users\me\"), &own),
        vec![("RUSTUP_HOME", r"C:\Users\me\tools\rustup".to_owned())]
    );
    assert_eq!(rust(&|_| true, &|_| Some(String::new())).len(), 2, "the defaults");
}

/// Variables that move a protected store name what is protected
/// there; the cargo home is offered only once its credential
/// files are on record.
#[test]
fn stores_moved_by_the_environment_are_named() {
    let env = |var: &str| match var {
        "CARGO_HOME" => Some(r"C:\Users\me\tools\cargo\".to_owned()),
        "GRADLE_USER_HOME" => Some(r"C:\Users\me\g".to_owned()),
        "GNUPGHOME" => Some("C:/Users/me/keys".to_owned()),
        "KUBECONFIG" => Some(r"C:\Users\me\k\a.yaml;;D:\k\b.yaml; ".to_owned()),
        "NETRC" => Some(String::new()),
        _ => None,
    };
    assert_eq!(
        env_exclusions(&env).0,
        [
            r"C:\Users\me\tools\cargo\credentials",
            r"C:\Users\me\tools\cargo\credentials.toml",
            r"C:\Users\me\g\enterprise\keys.properties",
            r"C:\Users\me\g\develocity\keys.properties",
            r"C:\Users\me\keys",
            r"C:\Users\me\k\a.yaml",
            r"D:\k\b.yaml",
        ]
    );
    assert!(env_exclusions(&|_| None).0.is_empty());
    // A relative value names nothing fixed: ignored, and said.
    let relative = |var: &str| match var {
        "KUBECONFIG" => Some(r"kubeconfig;C:\k\a.yaml;.kube\b".to_owned()),
        "CARGO_HOME" => Some("cargo".to_owned()),
        "NETRC" => Some(r"C:netrc".to_owned()),
        "GNUPGHOME" => Some(r"\\srv\share\gnupg".to_owned()),
        _ => None,
    };
    let (kept, ignored) = env_exclusions(&relative);
    assert_eq!(kept, [r"\\srv\share\gnupg", r"C:\k\a.yaml"]);
    assert_eq!(
        ignored,
        [
            "CARGO_HOME=cargo",
            "NETRC=C:netrc",
            "KUBECONFIG=kubeconfig",
            r"KUBECONFIG=.kube\b"
        ]
    );
    for (var, _) in ENV_STORES {
        assert!(!["RUSTUP_HOME", "PIP_CONFIG_FILE", "NPM_CONFIG_USERCONFIG"].contains(var));
    }
    let recorded = vec![
        r"\\?\C:\Users\me\.cargo\credentials".to_owned(),
        r"\\?\C:\Users\me\.cargo\credentials.toml".to_owned(),
        r"\\?\C:\Users\me\tools\cargo\credentials.toml".to_owned(),
    ];
    assert!(cargo_home_protected(&recorded, r"C:\Users\ME\.cargo"));
    assert!(cargo_home_protected(&recorded, r"C:\Users\me\.cargo\"));
    assert!(!cargo_home_protected(&recorded, r"C:\Users\me\tools\cargo"));
    assert!(!cargo_home_protected(&recorded, r"C:\Users\me\other"));
    assert!(!cargo_home_protected(&[], r"C:\Users\me\.cargo"));
}

/// `.rustup` is granted whole; `.cargo` is a container whose credential
/// files are denied on themselves.
#[test]
fn a_container_is_traversable_and_its_exclusions_stay_denied() -> Result<(), Box<dyn std::error::Error>> {
    let mut t = Mem::default();
    t.add(r".cargo\credentials.toml")
        .add(r".cargo\config.toml")
        .add(r".cargo\registry\index\x")
        .add(r".rustup\toolchains\stable\bin\cargo.exe");
    let k = kinds(&plan(&mut t, &rules())?);
    let kind = |p: &str| k.get(p).map(|v| v.0);
    assert_eq!(kind(".rustup"), Some(Kind::Granted));
    assert_eq!(kind(".cargo"), Some(Kind::Container));
    assert_eq!(kind(r".cargo\registry"), Some(Kind::Granted));
    assert_eq!(kind(r".cargo\config.toml"), Some(Kind::File));
    assert_eq!(kind(r".cargo\credentials.toml"), Some(Kind::Denied));
    Ok(())
}

#[test]
fn a_session_grant_is_measured_against_the_recorded_exclusions() -> Result<(), Box<dyn std::error::Error>> {
    let p = |rel: &str| format!(r"\\?\C:\Users\me\{rel}");
    let protected = vec![
        p(".ssh"),
        p(r"AppData\Local\*\EBWebView"),
        p("NTUSER.DAT*"),
        p(r"AppData\Local\Temp\keys"),
    ];
    let ungranted = vec![p(r"AppData\Local\Temp")];
    let on_disk = |dir: &str| -> Listing {
        let d = dir.to_lowercase();
        let names: &[&str] = if d.ends_with(r"\appdata\local") {
            &["App", "Tool", "npm-cache", "Temp"]
        } else if d.ends_with(r"\appdata\local\app") {
            &["EBWebView", "data"]
        } else if d.ends_with(r"\appdata\local\tool") {
            &["bin"]
        } else if d.ends_with(r"\appdata") {
            &["Local", "Roaming"]
        } else if d.ends_with(r"\users\me") {
            &["AppData", "Documents"]
        } else {
            return Listing::Nothing;
        };
        Listing::Entries(names.iter().map(|n| (*n).to_owned()).collect())
    };
    let check = |rel: &str, write: bool, admitted: &[String]| {
        session_grant_refusal(&p(rel), write, &protected, &ungranted, admitted, &on_disk)
    };
    let refused = |rel: &str, write: bool| check(rel, write, &[]).is_some();
    for write in [false, true] {
        assert!(refused(".ssh", write));
        assert!(refused(r".SSH.\keys", write));
        assert!(refused(r"AppData\Local\App\EBWebView\Default", write));
        assert!(refused("ntuser.dat.LOG1", write));
        assert!(refused(r"AppData\Local\App", write));
        assert!(refused("AppData", write));
        assert!(!refused(r"AppData\Local\Tool", write));
        assert!(!refused(r"AppData\Local\npm-cache", write));
        assert!(!refused(r"AppData\Local\Tool\bin", write));
        assert!(refused(r"AppData\Local\Temp", write));
        assert!(refused(r"AppData\Local", write));
        assert!(!refused(r"AppData\Local\Temp\session", write));
        assert!(!refused(r"AppData\Local\Temp\session\work", write));
        assert!(refused(r"AppData\Local\Temp\keys", write));
        assert!(refused(r"AppData\Local\Temp\keys\sub", write));
        assert!(!refused("Documents", write));
        assert!(!refused(".sshx", write));
        assert!(!refused(r"AppData\Local\App\data", write));
    }
    for root in [r"\\?\C:\Users\me", r"\\?\C:\Users", r"\\?\C:\"] {
        for write in [false, true] {
            assert!(
                session_grant_refusal(root, write, &protected, &ungranted, &[], &on_disk).is_some(),
                "{root}"
            );
        }
    }
    for other in [r"\\?\D:\work", r"\\?\C:\Users\other\.ssh"] {
        assert_eq!(
            session_grant_refusal(other, true, &protected, &ungranted, &[], &on_disk),
            None
        );
    }
    let ssh = [p(".ssh")];
    assert_eq!(check(".ssh", false, &ssh), None);
    assert!(check(".ssh", true, &ssh).is_some());
    assert!(check(r".ssh\keys", false, &ssh).is_some());
    assert_eq!(check(r".ssh\keys", false, &[p(r".ssh\keys")]), None);
    assert!(check("AppData", false, &[p("AppData")]).is_some());
    let temp = [p(r"AppData\Local\Temp")];
    assert!(check(r"AppData\Local\Temp", false, &temp).is_some());
    let empty = |_: &str| Listing::Nothing;
    let bare = |rel: &str| session_grant_refusal(&p(rel), true, &protected, &ungranted, &[], &empty).is_some();
    assert!(!bare(r"AppData\Local\App"));
    assert!(bare(r"AppData\Local\App\EBWebView"));
    assert!(bare(r"AppData\Local"));
    assert!(bare(""));
    let deep = |dir: &str| -> Listing {
        let d = dir.to_lowercase();
        if d.ends_with(r"\local\x") {
            Listing::Entries(vec!["y".to_owned()])
        } else if d.ends_with(r"\local\x\y") {
            Listing::Entries(vec!["EBWebView".to_owned()])
        } else {
            Listing::Nothing
        }
    };
    assert_eq!(
        session_grant_refusal(&p(r"AppData\Local\x"), true, &protected, &ungranted, &[], &deep),
        None
    );
    let two = vec![p(r"apps\*\*\store")];
    let with = |at_apps: Listing, at_a: Listing| {
        move |dir: &str| -> Listing {
            let d = dir.to_lowercase();
            if d.ends_with(r"\apps") {
                at_apps.clone()
            } else if d.ends_with(r"\apps\a") {
                at_a.clone()
            } else {
                Listing::Nothing
            }
        }
    };
    let names = |n: &[&str]| Listing::Entries(n.iter().map(|s| (*s).to_owned()).collect());
    let verdict = |list: &dyn Fn(&str) -> Listing| session_grant_refusal(&p("apps"), true, &two, &[], &[], list);
    assert_eq!(verdict(&with(names(&["a"]), names(&["b"]))), None);
    assert_eq!(verdict(&with(names(&["a"]), Listing::Nothing)), None);
    assert_eq!(verdict(&with(Listing::Nothing, Listing::Nothing)), None);
    assert_eq!(verdict(&with(names(&["a"]), Listing::Link)), None);
    for (at_apps, at_a) in [
        (Listing::Failed("access denied".into()), Listing::Nothing),
        (Listing::Failed("i/o error".into()), Listing::Nothing),
        (names(&["a"]), Listing::Failed("access denied".into())),
        (names(&["a", "z"]), Listing::Failed("sharing violation".into())),
        (Listing::Link, Listing::Nothing),
    ] {
        let why = verdict(&with(at_apps.clone(), at_a.clone()))
            .ok_or_else(|| format!("{at_apps:?}/{at_a:?} was admitted"))?;
        assert!(why.contains("could not be examined"), "{why}");
    }
    let narrow = vec![p(r"apps\b\*\store")];
    assert_eq!(
        session_grant_refusal(
            &p("apps"),
            true,
            &narrow,
            &[],
            &[],
            &with(names(&["a"]), Listing::Failed("access denied".into()))
        ),
        None
    );
    let found = |dir: &str| -> Listing {
        let d = dir.to_lowercase();
        if d.ends_with(r"\apps") {
            names(&["a", "c"])
        } else if d.ends_with(r"\apps\a") {
            Listing::Failed("access denied".into())
        } else if d.ends_with(r"\apps\c") {
            names(&["d"])
        } else if d.ends_with(r"\apps\c\d") {
            names(&["store"])
        } else {
            Listing::Nothing
        }
    };
    let why = verdict(&found).ok_or("missing value")?;
    assert!(why.contains("narrower"), "{why}");
    let why = check("AppData", true, &[]).ok_or("missing value")?;
    assert!(why.contains("narrower"), "{why}");
    assert_eq!(relation(&p(".ssh"), &p(".ssh")), Some(Relation::Equal));
    assert_eq!(relation(&p(r".ssh\a"), &p(".ssh")), Some(Relation::Inside));
    assert_eq!(relation(&p(""), &p(".ssh")), Some(Relation::Above));
    assert_eq!(relation(&p("x"), &p(".ssh")), None);
    Ok(())
}

#[test]
fn a_read_deny_is_also_written_on_each_recorded_allow_beneath_it() {
    let root = r"\\?\C:\Users\me";
    let rec: Vec<(String, Kind)> = [
        ("", Kind::Container),
        (r"\Documents", Kind::Granted),
        (r"\.gitconfig", Kind::File),
        (r"\.ssh", Kind::Denied),
        (r"\AppData", Kind::Container),
        (r"\AppData\Local", Kind::Container),
        (r"\AppData\Local\Temp", Kind::Ungranted),
        (r"\AppData\Local\Programs", Kind::Granted),
        (r"\AppData\Local\IconCache.db", Kind::File),
        (r"\AppData\Roaming", Kind::Container),
        (r"\OneDrive", Kind::Skipped),
    ]
    .iter()
    .map(|(p, k)| (format!("{root}{p}"), *k))
    .collect();
    let also = |deny: &str| -> Vec<String> {
        covered_by_read_deny(&format!("{root}{deny}"), rec.iter().map(|(p, k)| (p.as_str(), *k)))
            .into_iter()
            .map(|p| p[root.len()..].to_owned())
            .collect()
    };
    assert_eq!(
        also(r"\AppData"),
        [
            r"\AppData\Local",
            r"\AppData\Local\Programs",
            r"\AppData\Local\IconCache.db",
            r"\AppData\Roaming"
        ]
    );
    assert_eq!(also(r"\APPDATA\local.").len(), 2);
    assert!(also(r"\Documents").is_empty());
    assert!(also(r"\.gitconfig").is_empty());
    assert!(also(r"\Documents\work\secret").is_empty());
    assert!(also(r"\App").is_empty());
    assert_eq!(also("").len(), 7);

    let many: Vec<(String, Kind)> = (0..6000)
        .map(|i| {
            let kind = match i % 3 {
                0 => Kind::Container,
                1 => Kind::Granted,
                _ => Kind::File,
            };
            (format!(r"{root}\AppData\Local\dir{}\entry-{i}", i % 40), kind)
        })
        .chain((0..500).map(|i| (format!(r"{root}\Documents\d{i}"), Kind::Granted)))
        .collect();
    let got = covered_by_read_deny(&format!(r"{root}\AppData"), many.iter().map(|(p, k)| (p.as_str(), *k)));
    assert_eq!(got.len(), 6000);
    assert!(got.iter().map(|p| p.len() + 14).sum::<usize>() > 32_767);
    let one = covered_by_read_deny(
        &format!(r"{root}\AppData\Local\dir7"),
        many.iter().map(|(p, k)| (p.as_str(), *k)),
    );
    assert_eq!(one.len(), 150);
}

/// The case table the library's tests read too
/// (`tests/data/read-deny-cases.txt`).
#[test]
fn read_denies_and_session_allows_rank_as_the_library_ranks_them() -> Result<(), Box<dyn std::error::Error>> {
    let table = include_str!("../../tests/data/read-deny-cases.txt");
    let path = |p: &str| -> Result<String, String> {
        let p = p.trim();
        match p.strip_prefix('P') {
            Some(rest) => Ok(format!(r"\\?\C:\Users\me{rest}")),
            None => Err(format!("'{p}' is not beneath P")),
        }
    };
    let list = |v: &str| -> Result<Vec<String>, String> {
        v.split(" | ")
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(path)
            .collect()
    };
    let mut recorded: Vec<(String, Kind)> = Vec::new();
    #[derive(Default)]
    struct Case {
        name: String,
        /// Handed read and write grants.
        allows: Vec<String>,
        /// What a per-command deny is sent with, where the case
        /// says: the reduced list of grants.
        except: Option<Vec<String>>,
        denies: Vec<String>,
        expect: Vec<String>,
    }
    let mut cases: Vec<Case> = Vec::new();
    for line in table.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, value) = line.split_once(':').ok_or("missing value")?;
        let value = value.trim();
        match key {
            "recorded" => {
                for item in value.split(" | ") {
                    let (kind, p) = item.trim().split_once(' ').ok_or("missing value")?;
                    recorded.push((path(p)?, Kind::parse(kind).ok_or("missing value")?));
                }
            }
            "case" => cases.push(Case {
                name: value.to_owned(),
                ..Default::default()
            }),
            "handed-read" | "handed-write" => cases.last_mut().ok_or("missing value")?.allows.extend(list(value)?),
            "handed-deny" => cases.last_mut().ok_or("missing value")?.denies = list(value)?,
            "except" => cases.last_mut().ok_or("missing value")?.except = Some(list(value)?),
            "expect" => cases.last_mut().ok_or("missing value")?.expect = list(value)?,
            "read" | "write" | "deny" | "credential" => {}
            other => return Err(format!("unknown key '{other}'").into()),
        }
    }
    assert!(cases.len() >= 12 && recorded.len() >= 10);
    let index = RecordedAllows::new(recorded.iter().map(|(p, k)| (p.as_str(), *k)));
    assert!(cases.iter().any(|c| c.except.is_some()));
    let runs = cases.iter().flat_map(|c| {
        std::iter::once((&c.name, &c.allows, &c.denies, &c.expect)).chain(
            c.except
                .as_ref()
                .map(|reduced| (&c.name, reduced, &c.denies, &c.expect)),
        )
    });
    for (name, allows, denies, expect) in runs {
        let mut got: Vec<String> = Vec::new();
        for d in denies {
            let d_norm = norm_path(d);
            got.extend(
                recorded
                    .iter()
                    .filter(|(p, k)| {
                        matches!(k, Kind::Container | Kind::Granted | Kind::File) && norm_path(p) == d_norm
                    })
                    .map(|(p, _)| p.clone()),
            );
            got.extend(index.covered(d, allows).into_iter().map(str::to_owned));
        }
        let sorted = |mut v: Vec<String>| {
            v.sort();
            v.dedup();
            v
        };
        assert_eq!(sorted(got), sorted(expect.clone()), "case: {name}");
    }
    Ok(())
}

/// What an object's own access list lets the sandbox account do,
/// and so what an exclusion is whose deny could not be written.
#[test]
fn an_unwritable_exclusion_is_judged_by_its_own_access_list() -> Result<(), Box<dyn std::error::Error>> {
    const USER: &str = "S-1-5-21-1-2-3-1001";
    const GROUP: &str = "S-1-5-21-1-2-3-1002";
    const OWNER: &str = "S-1-5-21-1-2-3-500";
    const SYSTEM: &str = "S-1-5-18";
    const ADMINS: &str = "S-1-5-32-544";
    const RX: u32 = 0x1200a9;
    const FULL: u32 = 0x1f01ff;
    let certain: Vec<&str> = [USER, GROUP].into_iter().chain(SANDBOX_ALWAYS).collect();
    let allow = |sid, mask| AceFact {
        deny: false,
        sid,
        mask,
        inherit_only: false,
        inherited_beneath: true,
        inherited: false,
    };
    let deny = |sid, mask| AceFact {
        deny: true,
        sid,
        mask,
        inherit_only: false,
        inherited_beneath: true,
        inherited: false,
    };
    let class = |dacl: &[AceFact<'_>]| unstamped_exclusion(Ok(Some(dacl)), &certain, &SANDBOX_MAYBE);

    assert_eq!(
        class(&[allow(OWNER, FULL), allow(SYSTEM, FULL), allow(ADMINS, FULL)]),
        Unstamped::SelfClosed
    );
    assert_eq!(
        class(&[
            deny("S-1-1-0", RX),
            deny("S-1-5-11", RX),
            deny("S-1-5-32-545", RX),
            allow(OWNER, FULL),
            allow("S-1-5-32-545", RX),
        ]),
        Unstamped::SelfClosed
    );
    assert_eq!(
        class(&[allow("S-1-1-0", RX), allow(SYSTEM, FULL), allow(ADMINS, FULL)]),
        Unstamped::Unprotected
    );
    for sid in certain.iter().chain(&SANDBOX_MAYBE) {
        assert_eq!(class(&[allow(sid, RX)]), Unstamped::Unprotected, "{sid}");
    }
    assert_eq!(
        class(&[allow("S-1-5-32-545", RX), deny("S-1-1-0", RX)]),
        Unstamped::Unprotected
    );
    assert_eq!(
        class(&[deny(GROUP, 0x1), allow("S-1-1-0", FULL)]),
        Unstamped::SelfClosed
    );
    assert_eq!(
        class(&[deny("S-1-5-4", RX), allow("S-1-1-0", RX)]),
        Unstamped::Unprotected
    );
    assert_eq!(
        class(&[deny("S-1-1-0", 0x10000), allow("S-1-1-0", 0x100080)]),
        Unstamped::SelfClosed
    );
    let only_beneath = |a: AceFact<'static>| AceFact {
        inherit_only: true,
        inherited_beneath: true,
        ..a
    };
    let only_here = |a: AceFact<'static>| AceFact {
        inherited_beneath: false,
        ..a
    };
    for sid in ["S-1-1-0", "S-1-5-11", "S-1-5-32-545", GROUP, "S-1-5-4"] {
        assert_eq!(
            class(&[allow(OWNER, FULL), only_beneath(allow(sid, RX))]),
            Unstamped::Unprotected,
            "{sid}"
        );
    }
    assert_eq!(
        class(&[only_here(deny("S-1-1-0", RX)), only_beneath(allow("S-1-5-32-545", RX)),]),
        Unstamped::Unprotected
    );
    assert_eq!(
        class(&[
            only_beneath(deny("S-1-1-0", RX)),
            only_beneath(allow("S-1-5-32-545", RX)),
            only_here(allow(OWNER, FULL)),
        ]),
        Unstamped::SelfClosed
    );
    assert_eq!(
        class(&[only_beneath(deny("S-1-1-0", RX)), only_here(allow("S-1-5-32-545", RX)),]),
        Unstamped::Unprotected
    );
    assert_eq!(
        class(&[only_beneath(allow(OWNER, FULL)), only_here(allow(SYSTEM, FULL))]),
        Unstamped::SelfClosed
    );
    assert_eq!(class(&[allow("S-1-1-0", 0x8000_0000)]), Unstamped::Unprotected);
    assert_eq!(class(&[allow("S-1-5-11", 0x1000_0000)]), Unstamped::Unprotected);
    let copy = |a: AceFact<'static>| AceFact { inherited: true, ..a };
    let list = [allow(OWNER, FULL), copy(allow(GROUP, RX)), allow("S-1-1-0", RX)];
    let by = reading_entry(&list, &certain, &SANDBOX_MAYBE).ok_or("missing value")?;
    assert!(by.sid == GROUP && by.inherited);
    let list = [deny("S-1-1-0", 0x10000), allow(GROUP, RX)];
    let by = reading_entry(&list, &certain, &SANDBOX_MAYBE).ok_or("missing value")?;
    assert!(by.sid == GROUP && !by.inherited);
    let list = [allow("S-1-5-32-545", RX), allow(GROUP, RX)];
    assert_eq!(
        reading_entry(&list, &certain, &SANDBOX_MAYBE)
            .ok_or("missing value")?
            .sid,
        "S-1-5-32-545"
    );
    assert!(reading_entry(&[allow(OWNER, FULL)], &certain, &SANDBOX_MAYBE).is_none());
    assert_eq!(class(&[]), Unstamped::SelfClosed);
    assert_eq!(
        unstamped_exclusion(Ok(None), &certain, &SANDBOX_MAYBE),
        Unstamped::Unprotected
    );
    assert_eq!(
        unstamped_exclusion(Err(()), &certain, &SANDBOX_MAYBE),
        Unstamped::Unverifiable
    );
    Ok(())
}

#[test]
fn kind_names_round_trip() {
    for k in [
        Kind::Container,
        Kind::Granted,
        Kind::File,
        Kind::Denied,
        Kind::Ungranted,
        Kind::Skipped,
    ] {
        assert_eq!(Kind::parse(k.as_str()), Some(k));
    }
    assert_eq!(Kind::parse("x"), None);
}
