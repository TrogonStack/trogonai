use std::collections::BTreeMap;

use trogon_sandbox_windows_fence::{
    Access, DirLook, ExclusionConflict, ExclusionKind, PathOnDisk, ProfileDisk, ProfileExclusions, Relation,
    exclusion_conflict_error, profile_exclusion_conflicts,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[derive(Default)]
struct Fake {
    dirs: BTreeMap<String, DirLook>,
    resolved: BTreeMap<String, PathOnDisk>,
}

impl Fake {
    fn dir(mut self, path: &str, names: &[&str]) -> Self {
        self.dirs.insert(
            path.to_lowercase(),
            DirLook::Names(names.iter().map(|n| n.to_string()).collect()),
        );
        self
    }

    fn look_as(mut self, path: &str, look: DirLook) -> Self {
        self.dirs.insert(path.to_lowercase(), look);
        self
    }

    fn real(mut self, path: &str, target: PathOnDisk) -> Self {
        self.resolved.insert(path.to_lowercase(), target);
        self
    }
}

impl ProfileDisk for Fake {
    fn resolve(&self, native: &str) -> PathOnDisk {
        self.resolved
            .get(&native.to_lowercase())
            .cloned()
            .unwrap_or_else(|| PathOnDisk::Resolved(native.to_string()))
    }

    fn look(&self, dir: &str) -> DirLook {
        self.dirs.get(&dir.to_lowercase()).cloned().unwrap_or(DirLook::Absent)
    }
}

fn grant() -> ProfileExclusions {
    ProfileExclusions {
        exclusions: vec![
            r"C:\Users\me\.ssh".to_string(),
            r"C:\Users\me\AppData\Roaming\*\secrets".to_string(),
        ],
        ungranted_only: vec![r"C:\Users\me\AppData".to_string()],
    }
}

fn conflicts(paths: &[&str], access: Access, admitted: &[&str], fake: &Fake) -> Vec<ExclusionConflict> {
    let paths: Vec<String> = paths.iter().map(|p| p.to_string()).collect();
    let admitted: Vec<String> = admitted.iter().map(|p| p.to_string()).collect();
    profile_exclusion_conflicts(&paths, &grant(), access, &admitted, fake)
}

const ROAMING: &str = r"C:\Users\me\AppData\Roaming";

#[test]
fn a_path_inside_an_excluded_directory_is_refused() -> TestResult {
    let found = conflicts(&[r"c:\users\ME\.ssh\keys"], Access::Read, &[], &Fake::default());
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].relation, Relation::Inside);
    assert_eq!(found[0].kind, ExclusionKind::Protected);
    Ok(())
}

#[test]
fn the_excluded_path_and_its_parent_are_refused() -> TestResult {
    let equal = conflicts(&[r"C:\Users\me\.ssh"], Access::Read, &[], &Fake::default());
    assert_eq!(equal[0].relation, Relation::Equal);
    let above = conflicts(&[r"C:\Users\me"], Access::Read, &[], &Fake::default());
    assert_eq!(above[0].relation, Relation::Above);
    Ok(())
}

#[test]
fn an_unrelated_path_is_not_refused() -> TestResult {
    assert!(conflicts(&[r"C:\work\repo"], Access::Read, &[], &Fake::default()).is_empty());
    Ok(())
}

#[test]
fn an_admitted_path_is_let_through_for_a_read_but_not_a_write() -> TestResult {
    let fake = Fake::default();
    let admitted = [r"C:\Users\me\.ssh"];
    assert!(conflicts(&[r"C:\Users\me\.ssh"], Access::Read, &admitted, &fake).is_empty());
    assert_eq!(
        conflicts(&[r"C:\Users\me\.ssh"], Access::Write, &admitted, &fake).len(),
        1
    );
    Ok(())
}

#[test]
fn an_admitted_path_is_still_refused_above_another_exclusion() -> TestResult {
    let found = conflicts(&[r"C:\Users\me"], Access::Read, &[r"C:\Users\me"], &Fake::default());
    assert_eq!(found[0].relation, Relation::Above);
    Ok(())
}

#[test]
fn an_ungranted_only_directory_is_refused_but_not_what_is_inside_it() -> TestResult {
    let fake = Fake::default();
    let equal = conflicts(&[r"C:\Users\me\AppData"], Access::Read, &[], &fake);
    assert_eq!(equal[0].kind, ExclusionKind::UngrantedOnly);
    assert!(conflicts(&[r"C:\Users\me\AppData\Local\tool"], Access::Read, &[], &fake).is_empty());
    Ok(())
}

#[test]
fn above_a_wildcard_exclusion_needs_a_match_on_disk() -> TestResult {
    let none = Fake::default().dir(ROAMING, &["app"]);
    assert!(conflicts(&[ROAMING], Access::Read, &[], &none).is_empty());
    let some = Fake::default()
        .dir(ROAMING, &["App"])
        .dir(r"C:\Users\me\AppData\Roaming\App", &["Secrets"]);
    let found = conflicts(&[ROAMING], Access::Read, &[], &some);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].unexamined, None);
    Ok(())
}

#[test]
fn a_wildcard_parent_that_cannot_be_listed_is_refused_with_the_reason() -> TestResult {
    let fake = Fake::default().real(ROAMING, PathOnDisk::Failed("EACCES".to_string()));
    let found = conflicts(&[ROAMING], Access::Read, &[], &fake);
    let reason = found[0].unexamined.as_deref().ok_or("expected a reason")?;
    assert!(reason.contains("EACCES"));
    let absent = Fake::default().real(ROAMING, PathOnDisk::Absent);
    assert!(conflicts(&[ROAMING], Access::Read, &[], &absent).is_empty());
    Ok(())
}

#[test]
fn a_link_into_an_excluded_directory_is_compared_by_where_it_leads() -> TestResult {
    let fake = Fake::default().real(
        r"C:\work\keys",
        PathOnDisk::Resolved(r"\\?\C:\Users\me\.ssh".to_string()),
    );
    let found = conflicts(&[r"C:\work\keys"], Access::Read, &[], &fake);
    assert_eq!(found[0].relation, Relation::Equal);
    Ok(())
}

#[test]
fn a_share_is_not_examined_above_a_wildcard() -> TestResult {
    let grant = ProfileExclusions {
        exclusions: vec![r"\\server\share\*\x".to_string()],
        ungranted_only: Vec::new(),
    };
    let found = profile_exclusion_conflicts(
        &[r"\\server\share".to_string()],
        &grant,
        Access::Read,
        &[],
        &Fake::default(),
    );
    let reason = found[0].unexamined.as_deref().ok_or("expected a reason")?;
    assert!(reason.contains("not on a local drive"));
    Ok(())
}

#[test]
fn the_message_names_the_list_and_the_relation() -> TestResult {
    let found = conflicts(&[r"C:\Users\me"], Access::Write, &[], &Fake::default());
    let message = exclusion_conflict_error(&found).message().to_string();
    assert!(message.contains(r"allowWrite C:\Users\me contains the excluded"));
    assert!(message.contains("choose a narrower working directory"));
    Ok(())
}

#[test]
fn the_message_describes_each_kind_and_relation() -> TestResult {
    let fake = Fake::default();
    let equal = conflicts(&[r"C:\Users\me\.ssh"], Access::Read, &[], &fake);
    assert!(
        exclusion_conflict_error(&equal)
            .message()
            .contains(r"allowRead C:\Users\me\.ssh is the excluded")
    );
    let inside = conflicts(&[r"C:\Users\me\.ssh\keys"], Access::Read, &[], &fake);
    assert!(
        exclusion_conflict_error(&inside)
            .message()
            .contains("lies inside the excluded")
    );
    let ungranted = conflicts(&[r"C:\Users\me\AppData"], Access::Read, &[], &fake);
    assert!(
        exclusion_conflict_error(&ungranted)
            .message()
            .contains("is not granted as a whole")
    );
    let above_ungranted = conflicts(&[r"C:\Users\me"], Access::Read, &[], &fake);
    assert!(
        exclusion_conflict_error(&above_ungranted)
            .message()
            .contains("contains")
    );
    let failing = Fake::default().real(ROAMING, PathOnDisk::Failed("EACCES".to_string()));
    let unexamined = conflicts(&[ROAMING], Access::Read, &[], &failing);
    assert!(
        exclusion_conflict_error(&unexamined)
            .message()
            .contains("may contain the excluded")
    );
    Ok(())
}

#[test]
fn a_wildcard_parent_that_is_not_a_plain_directory_is_not_matched() -> TestResult {
    for look in [DirLook::NotDirectory, DirLook::Absent] {
        let fake = Fake::default().look_as(ROAMING, look);
        assert!(conflicts(&[ROAMING], Access::Read, &[], &fake).is_empty());
    }
    let link = Fake::default().look_as(ROAMING, DirLook::Link);
    let found = conflicts(&[ROAMING], Access::Read, &[], &link);
    assert!(
        found[0]
            .unexamined
            .as_deref()
            .ok_or("expected a reason")?
            .contains("is a link")
    );
    let nested = Fake::default()
        .dir(ROAMING, &["App", "Other"])
        .look_as(r"C:\Users\me\AppData\Roaming\App", DirLook::Link)
        .look_as(r"C:\Users\me\AppData\Roaming\Other", DirLook::Failed("EIO".to_string()));
    let found = conflicts(&[ROAMING], Access::Read, &[], &nested);
    assert_eq!(found.len(), 1);
    let failed = Fake::default().look_as(ROAMING, DirLook::Failed("EIO".to_string()));
    let found = conflicts(&[ROAMING], Access::Read, &[], &failed);
    assert_eq!(found.len(), 1);
    assert!(found[0].unexamined.is_some());
    Ok(())
}
