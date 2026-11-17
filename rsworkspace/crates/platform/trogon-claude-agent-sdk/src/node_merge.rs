//! lodash `mergeWith` with the settings customizer (`ao` upstream) on a Node stack, which it spends on each array or
//! object of the source it enters, and more where V8 runs part of it for the first time or unoptimized.
//!
//! The merge writes into the destination as it goes. A value the destination already holds as an array or object is
//! merged into in place, while a fresh array or object is written only once the merge has filled it, so a merge that
//! stops keeps every value it wrote before and none of the fresh container it was filling.
//!
//! lodash reads each member through `safeGet`, which answers `undefined` for `__proto__`: an array or object under
//! that key merges nothing and is never entered, at any level of a fresh copy too, while a string, number, boolean
//! or `null` there is written as an own member. It walks an object's members as `for...in` does, array indices
//! first, and an array's as `keysIn` lists them: every index, holes included, then its named properties.
//!
//! An object merged onto an array sets elements, `length` and named properties of the array, and an array merged
//! onto an object with a valid `length` merges into `copyArray` of that object. Both leave holes and named
//! properties, which the merge keeps in band as [`js_array`](crate::js_array) describes.

use serde_json::{Map, Value};

use crate::{
    js_array::{JsArray, LengthError, Member, array_length, copy_array_like, fresh_copy, members, union},
    js_array_mark::ArrayMark,
    js_object_order::{array_index, js_key_order},
    node_merge_room::MergeRoom,
    node_merge_warmth::MergeWarmth,
    settings_merge_spec::MergeSpec,
    settings_resolve_error::ResolveSettingsError,
    settings_strings::array_mark,
};

/// How a merge on a Node stack ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum NodeMerge {
    /// Every array and object of the source fit the stack.
    Merged,
    /// The merge ran out of stack, which rejects with `RangeError: Maximum call stack size exceeded`.
    Overflowed,
    /// The merge set the `length` of an array to a value that is not a whole number from 0 to 2^32 - 1, or copied an
    /// object whose `length` passes 2^32 - 1, which throws `RangeError: Invalid array length`.
    InvalidArrayLength,
    /// The merge set the `length` of an array to an array that joins longer than a V8 string holds, which throws
    /// `RangeError: Invalid string length`.
    InvalidStringLength,
    /// The merge ran out of stack where V8 compiles the regular expression lodash `isIndex` tests array indexes
    /// with, which throws `SyntaxError: Invalid regular expression: /^(?:0|[1-9]\d*)$/: Stack overflow`.
    InvalidRegularExpression,
}

impl NodeMerge {
    /// What `resolveSettings` rejects with where nothing catches what stopped the merge.
    pub(crate) fn rejection(self) -> Option<ResolveSettingsError> {
        match self {
            Self::Merged => None,
            Self::Overflowed => Some(ResolveSettingsError::MaximumCallStack),
            Self::InvalidArrayLength => Some(ResolveSettingsError::InvalidArrayLength),
            Self::InvalidStringLength => Some(ResolveSettingsError::InvalidStringLength),
            Self::InvalidRegularExpression => Some(ResolveSettingsError::InvalidRegularExpression),
        }
    }
}

/// Why a merge stopped before it wrote every member.
enum Stop {
    Overflow,
    Length(LengthError),
    IndexCompile,
}

/// How lodash enters an array or object at a level.
#[derive(Clone, Copy, Eq, PartialEq)]
enum Level {
    /// The level fits the stack.
    Fits,
    /// The level does not fit, but lodash still enters it, as
    /// [`MergeCost::descent_reach`](crate::node_merge_cost::MergeCost::descent_reach) describes: it merges its
    /// members and runs out of stack as it returns from it.
    Doomed,
}

/// A source array or object a merge reads members from.
#[derive(Clone, Copy)]
enum Source<'a> {
    Object(&'a Map<String, Value>),
    Array(&'a [Value]),
}

impl<'a> Source<'a> {
    fn members(self, mark: ArrayMark) -> Vec<Member<'a>> {
        match self {
            Self::Object(entries) => js_key_order(entries)
                .into_iter()
                .map(|(name, value)| Member::Named(name, value))
                .collect(),
            Self::Array(items) => members(items, mark),
        }
    }
}

/// The key a merge reads and writes a member under: for an array, an index, its `length` or a named property.
#[derive(Clone, Copy, Eq, PartialEq)]
enum Key<'a> {
    Index(u64),
    Length,
    Name(&'a str),
}

impl Key<'_> {
    /// Whether `keys` lists this key, which only a named key can be.
    fn listed(self, keys: &[String]) -> bool {
        matches!(self, Self::Name(name) if keys.iter().any(|listed| listed == name))
    }
}

/// An array or object of the destination a merge writes members into.
enum Target<'v> {
    Object(&'v mut Map<String, Value>),
    Array(JsArray<'v>),
}

impl Target<'_> {
    fn key<'a>(&self, name: &'a str) -> Key<'a> {
        match self {
            Self::Object(_) => Key::Name(name),
            Self::Array(_) if name == "length" => Key::Length,
            Self::Array(_) => array_index(name).map_or(Key::Name(name), |index| Key::Index(u64::from(index))),
        }
    }

    /// The member under `key` as `safeGet` reads it: never under `__proto__`, and no element in a hole.
    fn get(&self, key: Key<'_>) -> Option<&Value> {
        match (self, key) {
            (_, Key::Name("__proto__") | Key::Length) | (Self::Object(_), Key::Index(_)) => None,
            (Self::Object(entries), Key::Name(name)) => entries.get(name),
            (Self::Array(array), Key::Index(index)) => array.get(index),
            (Self::Array(array), Key::Name(name)) => array.prop(name),
        }
    }

    fn get_mut(&mut self, key: Key<'_>) -> Option<&mut Value> {
        match (self, key) {
            (_, Key::Name("__proto__") | Key::Length) | (Self::Object(_), Key::Index(_)) => None,
            (Self::Object(entries), Key::Name(name)) => entries.get_mut(name),
            (Self::Array(array), Key::Index(index)) => array.get_mut(index),
            (Self::Array(array), Key::Name(name)) => array.prop_mut(name),
        }
    }

    /// `target[key] = value`, which for the `length` of an array sets `ToNumber(value)` as its length. An object
    /// only ever takes a named key: [`Target::key`] names every member of an object, and lodash `baseMergeDeep` merges
    /// a source array onto a held object through `copyArray`, so an object never takes the elements of an array.
    fn write(&mut self, key: Key<'_>, value: Value, mark: ArrayMark) -> Result<(), Stop> {
        match self {
            Self::Object(entries) => {
                if let Key::Name(name) = key {
                    entries.insert(name.to_owned(), value);
                }
            }
            Self::Array(array) => match key {
                Key::Index(index) => array.set(index, value),
                Key::Length => array.set_length(array_length(&value, mark).map_err(Stop::Length)?),
                Key::Name(name) => array.set_prop(name, value),
            },
        }
        Ok(())
    }

    /// `target[key] = undefined` for the holes or `undefined` elements of a source array up to `end`, which lodash
    /// writes only where the array holds no element. The target is the array the source array merges into.
    fn write_undefined(&mut self, end: u64) {
        if let Self::Array(array) = self {
            array.fill_undefined(end);
        }
    }
}

/// One step of a merge into a fresh array or object, in the order lodash takes them.
enum Step<'a> {
    /// Enter `entered` at `depth`, below `arrays` arrays of its path.
    Enter {
        entered: Source<'a>,
        depth: usize,
        arrays: usize,
    },
    /// Assign a member into the array or object at this depth.
    Assign(usize),
    /// Finish merging an array.
    FinishArray,
    /// Return from a [`Level::Doomed`] level, which runs out of stack.
    Leave,
}

fn container(value: &Value) -> Option<Source<'_>> {
    match value {
        Value::Object(entries) => Some(Source::Object(entries)),
        Value::Array(items) => Some(Source::Array(items)),
        _ => None,
    }
}

/// One merge: the settings customizer's rules, the stack it has, what V8 already ran of it, the mark of the arrays
/// it writes, and what the compile of the `isIndex` regular expression spends at the end of its reach.
struct Merge<'r, 'w> {
    rules: &'r MergeSpec,
    room: MergeRoom,
    warmth: &'w mut MergeWarmth,
    mark: ArrayMark,
    /// What the compile of the `isIndex` regular expression to machine code spends at the end of the descent reach:
    /// [`MergeCost::later_compile`](crate::node_merge_cost::MergeCost::later_compile) where an earlier merge
    /// compiled it to bytecode.
    reach_compile: usize,
}

/// Merges `source` into `destination` as lodash `mergeWith` with the settings customizer does with `room` on the
/// stack, after merges that left V8 as warm as `warmth` says, keeping what it wrote before it stops. `warmth` then
/// says what this merge ran too.
pub(crate) fn merge_within(
    rules: &MergeSpec,
    destination: &mut Map<String, Value>,
    source: &Map<String, Value>,
    room: MergeRoom,
    warmth: &mut MergeWarmth,
) -> NodeMerge {
    let reach_compile = if warmth.index_tests == 0 {
        0
    } else {
        room.cost.later_compile
    };
    let mut merge = Merge {
        rules,
        room,
        warmth,
        mark: array_mark(),
        reach_compile,
    };
    let mut target = Target::Object(destination);
    match merge
        .check(1, 0)
        .and_then(|()| merge.merge_members(&mut target, Source::Object(source), 1, 0))
    {
        Ok(()) => NodeMerge::Merged,
        Err(Stop::Overflow) => NodeMerge::Overflowed,
        Err(Stop::Length(LengthError::InvalidArrayLength)) => NodeMerge::InvalidArrayLength,
        Err(Stop::Length(LengthError::InvalidStringLength)) => NodeMerge::InvalidStringLength,
        Err(Stop::IndexCompile) => NodeMerge::InvalidRegularExpression,
    }
}

impl Merge<'_, '_> {
    fn check(&self, depth: usize, extra: usize) -> Result<(), Stop> {
        if self.room.fits(depth, extra) {
            Ok(())
        } else {
            Err(Stop::Overflow)
        }
    }

    /// Assigns a member into the array or object at `depth`.
    fn assign(&mut self, depth: usize) -> Result<(), Stop> {
        self.check(depth, self.room.cost.assign(*self.warmth))?;
        self.warmth.assigned = true;
        Ok(())
    }

    /// Enters an array or object at `depth` that spends nothing on top of its level.
    fn enter_level(&self, depth: usize) -> Result<Level, Stop> {
        if self.room.fits(depth, 0) {
            Ok(Level::Fits)
        } else if self.room.reaches(depth) {
            Ok(Level::Doomed)
        } else {
            Err(Stop::Overflow)
        }
    }

    /// Enters a source array holding `items` at `depth`, the `position`th array level of its path, which spends
    /// more on top of its levels as V8 is warm. lodash `isIndex` tests each index of an array that fits.
    ///
    /// An array holding members that does not fit where `isIndex` compiles its regular expression throws
    /// `SyntaxError` where V8 starts the compile: the bytecode compile where the stack holds the level and
    /// [`MergeCost::first_array`](crate::node_merge_cost::MergeCost::first_array), and the machine code compile
    /// where lodash still reaches the level and what the compile spends there.
    fn enter_array(&mut self, depth: usize, position: usize, items: &[Value]) -> Result<Level, Stop> {
        let extra = self.room.cost.array(position, !items.is_empty(), *self.warmth);
        if self.room.fits(depth, extra) {
            self.warmth.test_index_keys(items.len());
            return Ok(Level::Fits);
        }
        if !items.is_empty() && self.warmth.compiles_index_test() {
            let compiles = if self.warmth.index_tests == 0 {
                self.room.fits(depth, self.room.cost.first_array)
            } else {
                self.room.reaches_past(depth, self.reach_compile)
            };
            return Err(if compiles { Stop::IndexCompile } else { Stop::Overflow });
        }
        if extra == 0 {
            self.enter_level(depth)
        } else {
            Err(Stop::Overflow)
        }
    }

    /// Returns from a level the merge entered as `level`.
    fn leave(level: Level) -> Result<(), Stop> {
        match level {
            Level::Fits => Ok(()),
            Level::Doomed => Err(Stop::Overflow),
        }
    }

    /// Merges the members of `source`, an array or object the merge entered at `depth` below `arrays` arrays of its
    /// path, into `target` in their order.
    fn merge_members(
        &mut self,
        target: &mut Target<'_>,
        source: Source<'_>,
        depth: usize,
        arrays: usize,
    ) -> Result<(), Stop> {
        for member in source.members(self.mark) {
            let (key, value) = match member {
                Member::Undefined { end } => {
                    self.assign(depth)?;
                    target.write_undefined(end);
                    continue;
                }
                Member::Element(index, value) => (Key::Index(index), value),
                Member::Named(name, value) => (target.key(name), value),
            };
            self.merge_member(target, key, value, depth, arrays)?;
        }
        Ok(())
    }

    /// Merges `value`, the member under `key` of a source array or object entered at `depth`, into `target`.
    fn merge_member(
        &mut self,
        target: &mut Target<'_>,
        key: Key<'_>,
        value: &Value,
        depth: usize,
        arrays: usize,
    ) -> Result<(), Stop> {
        let rules = self.rules;
        let mark = self.mark;
        let next = depth.saturating_add(1);
        if key == Key::Name("__proto__") && container(value).is_some() {
            return Ok(());
        }
        let Some(child) = container(value).filter(|_| !key.listed(&rules.replaced)) else {
            self.assign(depth)?;
            return target.write(key, value.clone(), mark);
        };
        let copied = match (target.get(key), child) {
            (Some(Value::Object(held)), Source::Array(_)) => copy_array_like(held, mark).map_err(Stop::Length)?,
            _ => None,
        };
        if let (Some(mut copied), Source::Array(items)) = (copied, child) {
            let position = arrays.saturating_add(1);
            let level = self.enter_array(next, position, items)?;
            self.merge_members(
                &mut Target::Array(JsArray::new(&mut copied, mark)),
                child,
                next,
                position,
            )?;
            self.warmth.finished_array = true;
            Self::leave(level)?;
            self.assign(depth)?;
            return target.write(key, Value::Array(copied), mark);
        }
        let onto_fresh = match (target.get_mut(key), child) {
            (Some(Value::Array(held)), Source::Array(items)) => {
                self.assign(depth)?;
                *held = if key.listed(&rules.replaced_arrays) {
                    items.to_vec()
                } else {
                    union(held, items, mark)
                };
                false
            }
            (Some(Value::Object(held)), Source::Object(entries)) if key.listed(&rules.shallow_objects) => {
                self.assign(depth)?;
                for (name, item) in entries {
                    held.insert(name.clone(), item.clone());
                }
                false
            }
            (Some(Value::Object(held)), Source::Object(_)) => {
                let level = self.enter_level(next)?;
                self.merge_members(&mut Target::Object(held), child, next, arrays)?;
                Self::leave(level)?;
                self.assign(depth)?;
                false
            }
            (Some(Value::Array(held)), Source::Object(_)) => {
                let level = self.enter_level(next)?;
                self.merge_members(&mut Target::Array(JsArray::new(held, mark)), child, next, arrays)?;
                Self::leave(level)?;
                self.assign(depth)?;
                false
            }
            _ => true,
        };
        if onto_fresh {
            self.enter(child, next, arrays)?;
            self.assign(depth)?;
            target.write(key, fresh_copy(value, &rules.replaced, mark), mark)?;
        }
        Ok(())
    }

    /// Enters `entered` at `depth` onto a fresh array or object, which the merge fills but does not write until it
    /// is done. It enters every array and object of the source except under `__proto__` and under a key `rules`
    /// replaces.
    fn enter<'a>(&mut self, entered: Source<'a>, depth: usize, arrays: usize) -> Result<(), Stop> {
        let mut steps = vec![Step::Enter { entered, depth, arrays }];
        while let Some(step) = steps.pop() {
            let (entered, depth, arrays) = match step {
                Step::Assign(depth) => {
                    self.assign(depth)?;
                    continue;
                }
                Step::FinishArray => {
                    self.warmth.finished_array = true;
                    continue;
                }
                Step::Leave => return Err(Stop::Overflow),
                Step::Enter { entered, depth, arrays } => (entered, depth, arrays),
            };
            let (level, arrays) = match entered {
                Source::Object(_) => (self.enter_level(depth)?, arrays),
                Source::Array(items) => {
                    let position = arrays.saturating_add(1);
                    (self.enter_array(depth, position, items)?, position)
                }
            };
            if level == Level::Doomed {
                steps.push(Step::Leave);
            }
            if matches!(entered, Source::Array(_)) {
                steps.push(Step::FinishArray);
            }
            let first = steps.len();
            for member in entered.members(self.mark) {
                match member {
                    Member::Undefined { .. } => steps.push(Step::Assign(depth)),
                    Member::Element(index, value) => {
                        self.push_member(&mut steps, Key::Index(index), value, depth, arrays);
                    }
                    Member::Named(name, value) => self.push_member(&mut steps, Key::Name(name), value, depth, arrays),
                }
            }
            steps[first..].reverse();
        }
        Ok(())
    }

    /// Pushes, in their order, the steps of the member `value` under `key` of a fresh array or object entered at
    /// `depth` below `arrays` arrays of its path.
    fn push_member<'a>(&self, steps: &mut Vec<Step<'a>>, key: Key<'_>, value: &'a Value, depth: usize, arrays: usize) {
        match container(value) {
            Some(_) if key == Key::Name("__proto__") => return,
            Some(child) if !key.listed(&self.rules.replaced) => steps.push(Step::Enter {
                entered: child,
                depth: depth.saturating_add(1),
                arrays,
            }),
            _ => {}
        }
        steps.push(Step::Assign(depth));
    }
}

#[cfg(test)]
#[path = "node_merge_tests.rs"]
mod tests;
