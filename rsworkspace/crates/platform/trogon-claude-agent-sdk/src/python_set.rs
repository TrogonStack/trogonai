//! The iteration order of a CPython 3.12 `set` of strings (`Objects/setobject.c`): the order of its hash table, which
//! depends on the hashes of its keys, so on the hash secret, and on the order they went in.

use crate::constants::{LINEAR_PROBES, MIN_SIZE, PERTURB_SHIFT};
use crate::python_hash::{HashSecret, PythonHashed};

/// A set's hash table: each slot holds a key's hash and the key's index in the caller's list.
#[derive(Clone, Debug)]
struct Table {
    slots: Vec<Option<(u64, usize)>>,
    fill: usize,
    used: usize,
}

impl Default for Table {
    fn default() -> Self {
        Self {
            slots: vec![None; MIN_SIZE],
            fill: 0,
            used: 0,
        }
    }
}

impl Table {
    fn mask(&self) -> usize {
        self.slots.len() - 1
    }

    /// `set_add_entry`, with `same(index)` telling whether the key at `index` equals the one added.
    fn add(&mut self, hash: u64, index: usize, same: impl Fn(usize) -> bool) {
        let mask = self.mask();
        let mut perturb = hash;
        let mut i = start(hash, mask);
        loop {
            let probes = if i + LINEAR_PROBES <= mask { LINEAR_PROBES } else { 0 };
            for at in i..=i + probes {
                let slot = self.slots[at];
                match slot {
                    None => {
                        self.slots[at] = Some((hash, index));
                        self.fill += 1;
                        self.used += 1;
                        if self.fill * 5 >= mask * 3 {
                            let used = self.used;
                            self.resize(if used > 50_000 { used * 2 } else { used * 4 });
                        }
                        return;
                    }
                    Some((other, existing)) if other == hash && same(existing) => return,
                    Some(_) => {}
                }
            }
            perturb >>= PERTURB_SHIFT;
            i = next(i, perturb, mask);
        }
    }

    /// `set_table_resize`.
    fn resize(&mut self, min_used: usize) {
        let mut size = MIN_SIZE;
        while size <= min_used {
            size <<= 1;
        }
        if size == MIN_SIZE && self.slots.len() == MIN_SIZE && self.fill == self.used {
            return;
        }
        let old = std::mem::replace(&mut self.slots, vec![None; size]);
        for (hash, index) in old.into_iter().flatten() {
            self.insert_clean(hash, index);
        }
        self.fill = self.used;
    }

    /// `set_insert_clean`.
    fn insert_clean(&mut self, hash: u64, index: usize) {
        let mask = self.mask();
        let mut perturb = hash;
        let mut i = start(hash, mask);
        loop {
            if self.slots[i].is_none() {
                self.slots[i] = Some((hash, index));
                return;
            }
            if i + LINEAR_PROBES <= mask {
                for at in i + 1..=i + LINEAR_PROBES {
                    if self.slots[at].is_none() {
                        self.slots[at] = Some((hash, index));
                        return;
                    }
                }
            }
            perturb >>= PERTURB_SHIFT;
            i = next(i, perturb, mask);
        }
    }

    /// `set_merge` of this table into a new empty set.
    fn merged(&self) -> Self {
        let mut merged = Self::default();
        if self.used == 0 {
            return merged;
        }
        if (merged.fill + self.used) * 5 >= merged.mask() * 3 {
            merged.resize((merged.used + self.used) * 2);
        }
        if merged.mask() == self.mask() {
            merged.slots.clone_from(&self.slots);
        } else {
            for (hash, index) in self.slots.iter().flatten() {
                merged.insert_clean(*hash, *index);
            }
        }
        merged.fill = self.used;
        merged.used = self.used;
        merged
    }

    /// The indices of the keys in iteration order.
    fn order(&self) -> Vec<usize> {
        self.slots.iter().flatten().map(|(_, index)| *index).collect()
    }
}

/// `hash & mask`, with the hash as the `size_t` CPython uses.
fn start(hash: u64, mask: usize) -> usize {
    usize::try_from(hash & mask as u64).unwrap_or(0)
}

/// `(i * 5 + 1 + perturb) & mask`.
fn next(i: usize, perturb: u64, mask: usize) -> usize {
    let step = (i as u64).wrapping_mul(5).wrapping_add(1).wrapping_add(perturb);
    usize::try_from(step & mask as u64).unwrap_or(0)
}

fn hash_of<K: PythonHashed + ?Sized>(secret: HashSecret, key: &K) -> u64 {
    u64::from_ne_bytes(key.python_hash(secret).to_ne_bytes())
}

fn added<K: PythonHashed + PartialEq + ?Sized>(keys: &[&K], secret: HashSecret) -> Table {
    let mut table = Table::default();
    for (index, key) in keys.iter().enumerate() {
        table.add(hash_of(secret, *key), index, |existing| keys[existing] == *key);
    }
    table
}

/// The order `set(keys)` iterates `keys` in, as indices into `keys`, keeping the first of equal keys.
pub(crate) fn set_order<K: PythonHashed + PartialEq + ?Sized>(keys: &[&K], secret: HashSecret) -> Vec<usize> {
    added(keys, secret).order()
}

/// The order a set display of string constants iterates them in, as indices into `keys`. A display of fewer than
/// three items is `BUILD_SET`, which adds them in source order, so `keys` must be in source order then. A longer one
/// is `BUILD_SET 0` and `SET_UPDATE` of a constant `frozenset`, which `marshal` wrote to the `.pyc` in
/// `sorted(keys, key=marshal.dumps)` order and read back adding them in that order, so `keys` must be in that order
/// then.
pub(crate) fn display_order(keys: &[&str], secret: HashSecret) -> Vec<usize> {
    let table = added(keys, secret);
    if keys.len() < 3 {
        table.order()
    } else {
        table.merged().order()
    }
}

#[cfg(test)]
#[path = "python_set_tests.rs"]
mod tests;
