use std::cell::Cell;

use serde_json::{Map, Value, json};

use super::NodeStack;
use crate::{
    node_managed_pass::ManagedPass, node_merge::NodeMerge, node_merge_call::NodeMergeCall,
    node_merge_warmth::MergeWarmth, settings_merge_spec::MergeSpec, settings_resolve::on_engine_stack,
    settings_resolve_error::ResolveSettingsError,
};

/// A root object holding `objects - 1` nested objects under `x`, then `arrays` nested arrays, then `inner` nested
/// objects around `null`.
fn path(objects: usize, arrays: usize, inner: usize) -> Map<String, Value> {
    let wrap = |value: Value| Value::Object(Map::from_iter([("x".to_owned(), value)]));
    let mut value = Value::Null;
    for _ in 0..inner {
        value = wrap(value);
    }
    for _ in 0..arrays {
        value = Value::Array(vec![value]);
    }
    for _ in 1..objects {
        value = wrap(value);
    }
    Map::from_iter([("x".to_owned(), value)])
}

fn nested(objects: usize, arrays: usize) -> Map<String, Value> {
    path(objects, arrays, 0)
}

fn rules() -> MergeSpec {
    MergeSpec {
        replaced: vec!["modelPicker".to_owned()],
        replaced_arrays: vec!["fallbackModel".to_owned()],
        shallow_objects: vec!["extraKnownMarketplaces".to_owned(), "managedMcpServers".to_owned()],
    }
}

/// Whether merging `source` into a copy of `destination` in `call` runs out of `stack`, as the first merge of a
/// resolve.
fn overflows(
    stack: NodeStack,
    call: NodeMergeCall,
    rules: &MergeSpec,
    destination: &Map<String, Value>,
    source: &Map<String, Value>,
) -> bool {
    overflows_after(stack, call, (rules, destination, source), MergeWarmth::default())
}

/// Whether merging `source` into a copy of `destination` in `call` runs out of `stack`, after merges that left V8 as
/// warm as `warmth`.
fn overflows_after(
    stack: NodeStack,
    call: NodeMergeCall,
    (rules, destination, source): (&MergeSpec, &Map<String, Value>, &Map<String, Value>),
    warmth: MergeWarmth,
) -> bool {
    let mut destination = destination.clone();
    let warmth = Cell::new(warmth);
    stack.merge_within(call, rules, &mut destination, source, &warmth) == NodeMerge::Overflowed
}

#[test]
fn a_settings_file_reads_while_every_path_fits_both_walks_of_the_node_stack() -> Result<(), ResolveSettingsError> {
    on_engine_stack(|| {
        let stack = NodeStack::NODE_24_DARWIN_ARM64;
        for (objects, arrays, reads) in [
            (2223, 0, true),
            (2224, 0, false),
            (1, 3276, true),
            (1, 3277, false),
            (100, 3182, true),
            (100, 3183, false),
            (1688, 999, true),
            (1689, 999, false),
            (1448, 1447, true),
            (1449, 1447, false),
        ] {
            let document = Value::Object(nested(objects, arrays));
            assert_eq!(stack.reads(&document), reads, "{objects} objects and {arrays} arrays");
        }
        assert_eq!(stack.max_depth(), 3459);
        Ok(())
    })
}

#[test]
fn a_merge_overflows_past_the_nested_calls_the_node_stack_holds() -> Result<(), ResolveSettingsError> {
    on_engine_stack(|| {
        let stack = NodeStack::NODE_24_DARWIN_ARM64;
        let empty = Map::new();
        assert!(!overflows(
            stack,
            NodeMergeCall::Source,
            &rules(),
            &empty,
            &nested(1244, 0)
        ));
        assert!(overflows(
            stack,
            NodeMergeCall::Source,
            &rules(),
            &empty,
            &nested(1245, 0)
        ));
        assert!(!overflows(
            stack,
            NodeMergeCall::Source,
            &rules(),
            &empty,
            &nested(1, 1243)
        ));
        assert!(overflows(
            stack,
            NodeMergeCall::Source,
            &rules(),
            &empty,
            &nested(1, 1244)
        ));
        Ok(())
    })
}

#[test]
fn the_file_cache_copy_overflows_past_the_arrays_and_objects_the_node_stack_holds() -> Result<(), ResolveSettingsError>
{
    on_engine_stack(|| {
        let stack = NodeStack::NODE_24_DARWIN_ARM64;
        assert!(!stack.copy_overflows(&nested(1, 1945)));
        assert!(stack.copy_overflows(&nested(1, 1946)));
        assert!(!stack.copy_overflows(&path(1, 1886, 104)));
        assert!(stack.copy_overflows(&path(1, 1887, 104)));
        assert!(!stack.copy_overflows(&path(1, 1887, 0)));
        Ok(())
    })
}

#[test]
fn a_larger_startup_takes_levels_off_every_pass() -> Result<(), ResolveSettingsError> {
    on_engine_stack(|| {
        let roomy = NodeStack::NODE_24_DARWIN_ARM64.with_startup_bytes(727);
        let crowded = NodeStack::NODE_24_DARWIN_ARM64.with_startup_bytes(64_740);
        let empty = Map::new();
        for (stack, reads, merges) in [(roomy, 2231, 1249), (crowded, 2088, 1165)] {
            assert!(stack.reads(&Value::Object(nested(reads, 0))));
            assert!(!stack.reads(&Value::Object(nested(reads + 1, 0))));
            assert!(!overflows(
                stack,
                NodeMergeCall::Source,
                &rules(),
                &empty,
                &nested(merges, 0)
            ));
            assert!(overflows(
                stack,
                NodeMergeCall::Source,
                &rules(),
                &empty,
                &nested(merges + 1, 0)
            ));
        }
        Ok(())
    })
}

#[test]
fn a_merge_does_not_enter_what_the_settings_customizer_answers() -> Result<(), ResolveSettingsError> {
    on_engine_stack(|| {
        let stack = NodeStack::NODE_24_DARWIN_ARM64;
        let deep = nested(1, 3000);
        let onto_array = nested(1, 1);
        assert!(!overflows(stack, NodeMergeCall::Source, &rules(), &onto_array, &deep));
        let mut picker = Map::new();
        picker.insert("modelPicker".to_owned(), Value::Object(nested(2000, 0)));
        assert!(!overflows(stack, NodeMergeCall::Source, &rules(), &Map::new(), &picker));
        let mut spread = Map::new();
        spread.insert("managedMcpServers".to_owned(), Value::Object(nested(2000, 0)));
        let mut held = Map::new();
        held.insert("managedMcpServers".to_owned(), Value::Object(Map::new()));
        assert!(!overflows(stack, NodeMergeCall::Source, &rules(), &held, &spread));
        assert!(overflows(stack, NodeMergeCall::Source, &rules(), &Map::new(), &spread));
        Ok(())
    })
}

#[test]
fn a_managed_settings_document_reads_and_copies_within_the_budgets_of_each_evaluation()
-> Result<(), ResolveSettingsError> {
    on_engine_stack(|| {
        let stack = NodeStack::NODE_24_DARWIN_ARM64;
        for (pass, objects, arrays, copied, deepest) in [
            (ManagedPass::Sources, 2217, 3268, 1941, 3450),
            (ManagedPass::Origin, 2220, 3271, 1943, 3454),
            (ManagedPass::MachinePolicy, 2222, 3274, 1945, 3457),
        ] {
            assert!(stack.reads_policy(pass, &nested(objects, 0)), "{pass:?}");
            assert!(!stack.reads_policy(pass, &nested(objects + 1, 0)), "{pass:?}");
            assert!(stack.reads_policy(pass, &nested(1, arrays)), "{pass:?}");
            assert!(!stack.reads_policy(pass, &nested(1, arrays + 1)), "{pass:?}");
            assert!(!stack.policy_copy_overflows(pass, &nested(1, copied)), "{pass:?}");
            assert!(stack.policy_copy_overflows(pass, &nested(1, copied + 1)), "{pass:?}");
            assert_eq!(stack.policy_max_depth(pass), deepest, "{pass:?}");
        }
        Ok(())
    })
}

/// Whether merging `source` into a copy of `destination` in `call` runs out of `stack`, after merges that assigned a
/// member and finished an array.
fn warm_overflows(
    stack: NodeStack,
    call: NodeMergeCall,
    destination: &Map<String, Value>,
    source: &Map<String, Value>,
) -> bool {
    overflows_after(stack, call, (&rules(), destination, source), MergeWarmth::WARM)
}

#[test]
fn the_folder_and_admin_merges_start_on_their_own_stacks() -> Result<(), ResolveSettingsError> {
    on_engine_stack(|| {
        let stack = NodeStack::NODE_24_DARWIN_ARM64;
        let empty = Map::new();
        for (call, objects) in [
            (NodeMergeCall::Folder(ManagedPass::Sources), 1241),
            (NodeMergeCall::Folder(ManagedPass::Origin), 1243),
            (NodeMergeCall::Folder(ManagedPass::MachinePolicy), 1244),
            (NodeMergeCall::Admin, 1244),
        ] {
            assert!(!warm_overflows(stack, call, &empty, &nested(objects, 0)), "{call:?}");
            assert!(warm_overflows(stack, call, &empty, &nested(objects + 1, 0)), "{call:?}");
            assert!(
                !warm_overflows(stack, call, &empty, &nested(1, objects - 1)),
                "{call:?}"
            );
            assert!(warm_overflows(stack, call, &empty, &nested(1, objects)), "{call:?}");
        }
        Ok(())
    })
}

/// A root object holding `objects - 1` nested objects under `x` around `leaf`.
fn ending(objects: usize, leaf: Value) -> Value {
    let mut value = leaf;
    for _ in 1..objects {
        value = Value::Object(Map::from_iter([("x".to_owned(), value)]));
    }
    json!({ "x": value })
}

#[test]
fn the_reader_and_copy_spend_what_the_array_or_object_ending_a_path_holds() -> Result<(), ResolveSettingsError> {
    on_engine_stack(|| {
        let stack = NodeStack::NODE_24_DARWIN_ARM64;
        for (objects, leaf, reads) in [
            (2223, json!({}), true),
            (2224, json!({}), false),
            (2222, json!([]), true),
            (2223, json!([]), false),
            (2222, json!([true]), true),
            (2223, json!([true]), false),
            (2223, json!(true), true),
            (2224, json!(true), false),
        ] {
            assert_eq!(
                stack.reads(&ending(objects, leaf.clone())),
                reads,
                "{objects} objects around {leaf}"
            );
        }
        let arrays = |count: usize, leaf: Value| (0..count).fold(leaf, |value, _| Value::Array(vec![value]));
        let held = |value: Value| Map::from_iter([("x".to_owned(), value)]);
        for (count, leaf, overflows) in [
            (1944, json!({}), false),
            (1945, json!({}), true),
            (1944, json!({"a": null}), false),
            (1945, json!({"a": null}), true),
            (1945, json!(null), false),
            (1946, json!(null), true),
        ] {
            let settings = held(arrays(count, leaf.clone()));
            assert_eq!(
                stack.copy_overflows(&settings),
                overflows,
                "{count} arrays around {leaf}"
            );
        }
        Ok(())
    })
}

#[test]
fn a_merge_spends_more_at_the_first_member_it_assigns_and_the_first_arrays_it_enters()
-> Result<(), ResolveSettingsError> {
    on_engine_stack(|| {
        let stack = NodeStack::NODE_24_DARWIN_ARM64;
        let merges = |source: &Value| match source {
            Value::Object(source) => {
                let warmth = Cell::new(MergeWarmth::default());
                stack.merge_within(NodeMergeCall::Source, &rules(), &mut Map::new(), source, &warmth)
            }
            _ => NodeMerge::Overflowed,
        };
        let warm_first = |objects: usize, leaf: Value| {
            let mut source = Map::from_iter([("a".to_owned(), json!([]))]);
            if let Value::Object(path) = ending(objects, leaf) {
                source.extend(path);
            }
            Value::Object(source)
        };
        for (source, merged) in [
            (ending(1244, json!({})), NodeMerge::Merged),
            (ending(1245, json!({})), NodeMerge::Overflowed),
            (ending(1243, json!([])), NodeMerge::Merged),
            (ending(1244, json!([])), NodeMerge::Overflowed),
            (ending(1242, json!([null])), NodeMerge::Merged),
            (ending(1243, json!([null])), NodeMerge::InvalidRegularExpression),
            (warm_first(1244, json!([])), NodeMerge::Merged),
            (warm_first(1245, json!([])), NodeMerge::Overflowed),
            (warm_first(1245, json!(null)), NodeMerge::Merged),
            (warm_first(1246, json!(null)), NodeMerge::Overflowed),
        ] {
            assert_eq!(merges(&source), merged);
        }
        Ok(())
    })
}

#[test]
fn the_merges_of_a_resolve_share_how_many_keys_lodash_tested_as_indexes() -> Result<(), ResolveSettingsError> {
    on_engine_stack(|| {
        let stack = NodeStack::NODE_24_DARWIN_ARM64;
        let warmth = Cell::new(MergeWarmth::default());
        let mut effective = Map::new();
        let held = Map::from_iter([("u".to_owned(), json!([1])), ("v".to_owned(), json!([1]))]);
        let merged = stack.merge_within(NodeMergeCall::Source, &rules(), &mut effective, &held, &warmth);
        assert_eq!(merged, NodeMerge::Merged);
        assert_eq!(warmth.get().index_tests, MergeWarmth::INDEX_COMPILES);
        let compiled = warmth.get();
        for (objects, overflows) in [(1243, false), (1244, true)] {
            let source = match ending(objects, json!([null])) {
                Value::Object(source) => source,
                _ => Map::new(),
            };
            let call = NodeMergeCall::Folder(ManagedPass::MachinePolicy);
            assert_eq!(
                overflows_after(stack, call, (&rules(), &Map::new(), &source), compiled),
                overflows,
                "{objects} objects around [null]"
            );
        }
        Ok(())
    })
}

#[test]
fn the_command_line_and_environment_take_nothing_from_a_linux_profile() -> Result<(), ResolveSettingsError> {
    on_engine_stack(|| {
        let empty = Map::new();
        for (profile, reads, merges) in [
            (NodeStack::NODE_24_LINUX_ARM64, 1957, 1252),
            (NodeStack::NODE_24_LINUX_X64, 1649, 1307),
        ] {
            for startup in [0, 4384, 64_740] {
                let stack = profile.with_startup_bytes(startup);
                assert!(stack.reads(&Value::Object(nested(reads, 0))));
                assert!(!stack.reads(&Value::Object(nested(reads + 1, 0))));
                assert!(!overflows(
                    stack,
                    NodeMergeCall::Source,
                    &rules(),
                    &empty,
                    &nested(merges, 0)
                ));
                assert!(overflows(
                    stack,
                    NodeMergeCall::Source,
                    &rules(),
                    &empty,
                    &nested(merges + 1, 0)
                ));
            }
        }
        Ok(())
    })
}

#[test]
fn the_default_profile_is_the_one_for_the_platform_the_crate_is_built_for() {
    let expected = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "aarch64") => NodeStack::NODE_24_LINUX_ARM64,
        ("linux", "x86_64") => NodeStack::NODE_24_LINUX_X64,
        _ => NodeStack::NODE_24_DARWIN_ARM64,
    };
    assert_eq!(
        (NodeStack::for_build_target(), NodeStack::default()),
        (expected, expected)
    );
}
