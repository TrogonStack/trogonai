use super::{ConfinedPath, PathVetoError, RootPolicyError};

/// Confines the file tools to a set of in-sandbox directories. Absolute paths must sit under a root; relative
/// paths resolve against the first root. `..` is refused outright and every component below the root must not be
/// a symlink. With no roots configured nothing is checked.
///
/// The symlink check inspects the tree before the operation runs, so it is a guard against an agent wandering out
/// of its workspace, not a defence against a process inside the sandbox racing the check. It fails closed: a
/// provider that cannot answer `stat` refuses every call. Workers built on `trogon-sandbox-file-ops` re-check
/// where an opened descriptor landed against the sandbox root, so a raced path cannot reach the host, but a race
/// can still move a call to another directory inside the sandbox. Providers whose filesystem we reach only
/// through their own API get this check and nothing else.
#[derive(Debug, Clone, Default)]
pub struct RootPolicy {
    roots: Vec<Vec<String>>,
}

fn components(path: &str) -> Vec<&str> {
    path.split('/').filter(|c| !c.is_empty() && *c != ".").collect()
}

impl RootPolicy {
    pub fn new(roots: &[String]) -> Result<Self, RootPolicyError> {
        let mut parsed = Vec::new();
        for root in roots {
            if !root.starts_with('/') {
                return Err(RootPolicyError::NotAbsolute(root.clone()));
            }
            if root.split('/').any(|c| c == "..") {
                return Err(RootPolicyError::ParentComponent(root.clone()));
            }
            parsed.push(components(root).into_iter().map(str::to_string).collect());
        }
        Ok(Self { roots: parsed })
    }

    pub fn is_empty(&self) -> bool {
        self.roots.is_empty()
    }

    /// Lexical check only. Returns the normalized path and the prefixes to inspect for symlinks.
    pub fn confine(&self, path: &str) -> Result<ConfinedPath, PathVetoError> {
        if path.is_empty() {
            return Err(PathVetoError::Empty);
        }
        if path.split('/').any(|c| c == "..") {
            return Err(PathVetoError::ParentComponent(path.to_string()));
        }
        let wanted: Vec<String> = if path.starts_with('/') {
            components(path).into_iter().map(str::to_string).collect()
        } else {
            let first = self
                .roots
                .first()
                .ok_or_else(|| PathVetoError::OutsideRoots(path.to_string()))?;
            first
                .iter()
                .map(String::as_str)
                .chain(components(path))
                .map(str::to_string)
                .collect()
        };
        let root = self
            .roots
            .iter()
            .filter(|root| wanted.starts_with(root))
            .max_by_key(|root| root.len())
            .ok_or_else(|| PathVetoError::OutsideRoots(path.to_string()))?;
        let join = |parts: &[String]| format!("/{}", parts.join("/"));
        let below_root = (root.len() + 1..=wanted.len()).map(|n| join(&wanted[..n])).collect();
        Ok(ConfinedPath {
            path: join(&wanted),
            below_root,
        })
    }
}

#[cfg(test)]
mod tests;
