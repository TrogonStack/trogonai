//! The hook sanitizer of the settings read path (`p7e` and `f7e` upstream).
//!
//! It drops hook declarations that cannot load, except where a bad entry would silently disable a hook
//! that gates a tool call: those make the whole file unreadable instead.

use serde_json::{Map, Value};

use crate::{settings_read_spec::ReadSpec, settings_schema::SettingsSchema};

fn is_object(value: &Value) -> bool {
    value.is_object()
}

/// `C6`: a guard event key holds something other than nothing or an empty list.
fn holds_guard_events(value: &Value, spec: &ReadSpec) -> bool {
    let Value::Object(entries) = value else {
        return false;
    };
    entries.iter().any(|(key, entry)| {
        spec.guard_events.contains(key) && !entry.is_null() && !matches!(entry, Value::Array(items) if items.is_empty())
    })
}

/// `kS`: the value looks like one matcher where a map of events was expected.
fn is_single_matcher(value: &Value, spec: &ReadSpec) -> bool {
    let Value::Object(entries) = value else {
        return false;
    };
    if !entries.contains_key("matcher") && entries.keys().any(|key| spec.hook_events.contains(key)) {
        return false;
    }
    match entries.get("hooks") {
        Some(Value::Array(items)) => !items.is_empty(),
        Some(Value::Object(inner)) => {
            entries.contains_key("matcher") || inner.get("type").is_some_and(Value::is_string)
        }
        _ => false,
    }
}

/// How `rl` scans: the keys it does not enter and whether a matcher shape counts as a hit.
#[derive(Clone, Copy)]
struct Scan<'a> {
    unscanned: &'a [String],
    matchers_count: bool,
    in_hooks_list: bool,
}

/// `rl`: guard hooks hide inside the value, at most `depth` levels down.
fn hides_guard_hooks(value: &Value, depth: u32, scan: Scan<'_>, spec: &ReadSpec) -> bool {
    if holds_guard_events(value, spec) {
        return true;
    }
    if scan.matchers_count && is_single_matcher(value, spec) {
        return true;
    }
    if depth == 0 {
        return false;
    }
    match value {
        Value::Array(items) => items.iter().any(|item| hides_guard_hooks(item, depth - 1, scan, spec)),
        Value::Object(entries) => {
            if scan.in_hooks_list && entries.get("type").is_some_and(Value::is_string) {
                return false;
            }
            entries.iter().any(|(key, entry)| {
                !scan.unscanned.contains(key)
                    && hides_guard_hooks(
                        entry,
                        depth - 1,
                        Scan {
                            unscanned: scan.unscanned,
                            matchers_count: scan.matchers_count && !spec.hook_events.contains(key),
                            in_hooks_list: key == "hooks" && entry.is_array(),
                        },
                        spec,
                    )
            })
        }
        _ => false,
    }
}

/// `Sk`.
fn hides_matcher_or_guard(value: &Value, spec: &ReadSpec) -> bool {
    if is_single_matcher(value, spec) {
        return true;
    }
    match value {
        Value::Array(items) => items.iter().any(|item| hides_matcher_or_guard(item, spec)),
        Value::Object(_) => holds_guard_events(value, spec),
        _ => false,
    }
}

/// `E6`: guard hooks declared at the top level or under some other key.
pub(crate) fn declares_guard_hooks_outside_hooks(document: &Map<String, Value>, spec: &ReadSpec) -> bool {
    let whole = Value::Object(document.clone());
    if holds_guard_events(&whole, spec) {
        return true;
    }
    document.iter().any(|(key, entry)| {
        key != "hooks"
            && !spec.unscanned.contains(key)
            && hides_guard_hooks(
                entry,
                3,
                Scan {
                    unscanned: &spec.unscanned,
                    matchers_count: !spec.hook_events.contains(key),
                    in_hooks_list: false,
                },
                spec,
            )
    })
}

/// `G3e`: whether a hook entry has a problem that makes it unloadable.
fn entry_has_problem(entry: &Value, schema: &SettingsSchema) -> bool {
    let spec = schema.read_spec();
    let Value::Object(fields) = entry else {
        return true;
    };
    match fields.get("type") {
        Some(Value::String(kind)) if spec.hook_types.contains(kind) => !schema.root_accepts("hookEntry", entry),
        _ => true,
    }
}

/// What `A6` found in one event's matcher list.
struct EventScan {
    /// Something was spliced out or flagged.
    reported: bool,
    /// A problem upstream refuses to ignore.
    fatal: bool,
}

/// `A6`: removes unloadable matchers and entries from one event, from the end backwards. Guard events are
/// never changed; any problem there is fatal.
fn sanitize_event(list: &mut Value, event: &str, schema: &SettingsSchema) -> EventScan {
    let spec = schema.read_spec();
    let Value::Array(matchers) = list else {
        return EventScan {
            reported: false,
            fatal: false,
        };
    };
    let guard = spec.guard_events.iter().any(|name| name == event);
    let (mut problems, mut hidden_guards) = (0usize, 0usize);
    for index in (0..matchers.len()).rev() {
        let scan = Scan {
            unscanned: &[],
            matchers_count: false,
            in_hooks_list: false,
        };
        if hides_guard_hooks(&matchers[index], 3, scan, spec) {
            if guard {
                problems += 1;
            } else {
                hidden_guards += 1;
            }
            continue;
        }
        let drop_matcher = |matchers: &mut Vec<Value>, problems: &mut usize| {
            if !guard {
                matchers.remove(index);
            }
            *problems += 1;
        };
        let Value::Object(matcher) = &mut matchers[index] else {
            drop_matcher(matchers, &mut problems);
            continue;
        };
        let Some(Value::Array(entries)) = matcher.get_mut("hooks") else {
            drop_matcher(matchers, &mut problems);
            continue;
        };
        let mut entry_problems = 0usize;
        for position in (0..entries.len()).rev() {
            if entry_has_problem(&entries[position], schema) {
                if !guard {
                    entries.remove(position);
                }
                entry_problems += 1;
            }
        }
        let candidate = if guard {
            let mut copy = matcher.clone();
            copy.insert("hooks".to_owned(), Value::Array(Vec::new()));
            Value::Object(copy)
        } else {
            Value::Object(matcher.clone())
        };
        if !schema.root_accepts("hookMatcher", &candidate) {
            drop_matcher(matchers, &mut problems);
            continue;
        }
        problems += entry_problems;
    }
    EventScan {
        reported: problems > 0 || hidden_guards > 0,
        fatal: if guard { problems > 0 } else { hidden_guards > 0 },
    }
}

/// `f7e`: returns whether the document must be dropped.
pub(crate) fn sanitize_hooks(document: &mut Map<String, Value>, schema: &SettingsSchema) -> bool {
    let spec = schema.read_spec();
    let Some(hooks) = document.get_mut("hooks") else {
        return false;
    };
    let Value::Object(events) = hooks else {
        let keep_for_the_schema = match hooks {
            Value::Array(items) => items.iter().any(is_object) || hides_matcher_or_guard(hooks, spec),
            _ => false,
        };
        if !keep_for_the_schema {
            document.shift_remove("hooks");
        }
        return false;
    };
    if is_single_matcher(&Value::Object(events.clone()), spec) {
        return false;
    }
    let mut reported = false;
    let names: Vec<String> = events.keys().cloned().collect();
    for name in names {
        let Some(entry) = events.get(&name) else {
            continue;
        };
        let is_array = entry.is_array();
        let scan = Scan {
            unscanned: &[],
            matchers_count: !is_array,
            in_hooks_list: false,
        };
        if !spec.hook_events.contains(&name) {
            reported = true;
            if !hides_guard_hooks(entry, 3, scan, spec) {
                events.shift_remove(&name);
            }
            continue;
        }
        if !is_array {
            reported = true;
            let guard_value = spec.guard_events.contains(&name) && !entry.is_null();
            let scan = Scan {
                matchers_count: false,
                ..scan
            };
            if !guard_value && !hides_guard_hooks(entry, 3, scan, spec) {
                events.shift_remove(&name);
            }
        }
    }
    let mut fatal = false;
    for (name, list) in events.iter_mut() {
        let outcome = sanitize_event(list, name, schema);
        reported |= outcome.reported;
        fatal |= outcome.fatal;
    }
    if reported && events.is_empty() {
        document.shift_remove("hooks");
    }
    fatal
}
