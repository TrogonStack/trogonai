use std::collections::{BTreeMap, HashMap, HashSet};

use trogon_sandbox_contract::{Digest, DormancyArtifact, LeaseRecord, LeaseStatus, SnapshotKvRecord};

use super::listed_artifact::ListedArtifact;

pub fn lease_restore_sources(leases: &[LeaseRecord]) -> HashSet<Digest> {
    let mut out = HashSet::new();
    for lease in leases {
        let live = !matches!(lease.status, LeaseStatus::Released | LeaseStatus::Failed);
        if !live && lease.name.is_none() {
            continue;
        }
        if let Some(digest) = &lease.restored_from {
            out.insert(digest.clone());
        }
        if let Some(dormancy) = &lease.dormancy
            && let Some(DormancyArtifact::Remote(artifact)) = &dormancy.artifact
        {
            out.insert(artifact.digest.clone());
        }
    }
    out
}

/// Adds every ancestor of `roots` found in `catalog` to the set.
pub fn close_over_parents(roots: HashSet<Digest>, catalog: &[SnapshotKvRecord]) -> HashSet<Digest> {
    let mut parents: HashMap<&Digest, Vec<&Digest>> = HashMap::new();
    for record in catalog {
        if let Some(parent) = &record.parent {
            parents.entry(&record.digest).or_default().push(parent);
        }
    }
    let mut out = roots;
    let mut frontier: Vec<Digest> = out.iter().cloned().collect();
    while let Some(digest) = frontier.pop() {
        if let Some(edges) = parents.get(&digest) {
            for parent in edges {
                if out.insert((*parent).clone()) {
                    frontier.push((*parent).clone());
                }
            }
        }
    }
    out
}

/// Records beyond the newest `keep` of each (tenant, source lease) that are not
/// protected. `keep == 0` disables the pass.
pub fn plan_keep_n(records: &[SnapshotKvRecord], keep: usize, protected: &HashSet<Digest>) -> Vec<SnapshotKvRecord> {
    if keep == 0 {
        return Vec::new();
    }
    let mut groups: BTreeMap<(String, String), Vec<&SnapshotKvRecord>> = BTreeMap::new();
    for record in records {
        groups
            .entry((record.tenant.as_str().to_owned(), record.lease_id.as_str().to_owned()))
            .or_default()
            .push(record);
    }
    let mut out = Vec::new();
    for (_, mut group) in groups {
        // Newest first; unknown age sorts as oldest so it uses budget last.
        group.sort_by_key(|a| std::cmp::Reverse(a.created_at));
        for record in group.into_iter().skip(keep) {
            if record.created_at.is_some() && !protected.contains(&record.digest) {
                out.push(record.clone());
            }
        }
    }
    out
}

/// Listed artifacts the catalog has no record of and nothing protects.
pub fn find_orphans(
    listed: &[ListedArtifact],
    catalog: &[SnapshotKvRecord],
    protected: &HashSet<Digest>,
) -> Vec<ListedArtifact> {
    let known: HashSet<_> = catalog.iter().map(|r| (&r.tenant, &r.digest)).collect();
    listed
        .iter()
        .filter(|a| !known.contains(&(&a.tenant, &a.reference.digest)) && !protected.contains(&a.reference.digest))
        .cloned()
        .collect()
}
