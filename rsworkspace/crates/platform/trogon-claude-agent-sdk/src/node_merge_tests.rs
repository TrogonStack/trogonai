use serde_json::{Map, Value, json};

use super::{NodeMerge, merge_within};
use crate::{
    constants::V8_MAX_STRING_LENGTH, js_array_mark::ArrayMark, node_merge_cost::MergeCost, node_merge_room::MergeRoom,
    node_merge_warmth::MergeWarmth, settings_merge_spec::MergeSpec, settings_resolve_error::ResolveSettingsError,
};

const COST: MergeCost = MergeCost {
    level: 768,
    cold_assign: 416,
    first_array: 576,
    shallow_array: 1339,
    descent_reach: 40230,
    later_compile: 0,
};

/// Room for `levels` levels of a warm merge.
fn room(levels: usize) -> MergeRoom {
    MergeRoom {
        cost: COST,
        available: levels * COST.level,
    }
}

/// Merges `source` into `destination` with `available` bytes after merges that left V8 as warm as `warmth`.
fn merged(warmth: MergeWarmth, available: usize, destination: &mut Map<String, Value>, source: &Value) -> NodeMerge {
    let mut warmth = warmth;
    let room = MergeRoom { cost: COST, available };
    merge_within(&rules(), destination, &object(source.clone()), room, &mut warmth)
}

fn rules() -> MergeSpec {
    MergeSpec {
        replaced: vec!["modelPicker".to_owned()],
        replaced_arrays: vec!["fallbackModel".to_owned()],
        shallow_objects: vec!["managedMcpServers".to_owned()],
    }
}

fn object(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(entries) => entries,
        _ => Map::new(),
    }
}

/// `levels` nested objects under `x`, around `null`.
fn deep(levels: usize) -> Value {
    let mut value = Value::Null;
    for _ in 0..levels {
        value = Value::Object(Map::from_iter([("x".to_owned(), value)]));
    }
    value
}

#[test]
fn a_merge_that_runs_out_keeps_what_it_wrote_and_none_of_the_fresh_value_it_was_filling() {
    let mut destination = object(json!({"p": {"a": 1}}));
    let mut source = object(json!({"r": 1, "p": {"q": {"k": 1}, "z": null, "w": 2}, "s": 1}));
    if let Some(Value::Object(p)) = source.get_mut("p") {
        p.insert("z".to_owned(), deep(1300));
    }
    let mut warmth = MergeWarmth::WARM;
    assert_eq!(
        merge_within(&rules(), &mut destination, &source, room(1241), &mut warmth),
        NodeMerge::Overflowed
    );
    assert_eq!(
        Value::Object(destination),
        json!({"p": {"a": 1, "q": {"k": 1}}, "r": 1})
    );
}

#[test]
fn a_merge_that_fits_writes_every_member_as_the_settings_customizer_says() {
    let mut destination = object(json!({
        "list": [1, 2],
        "fallbackModel": ["a"],
        "managedMcpServers": {"one": {"keep": true}},
        "modelPicker": {"old": 1},
        "__proto__": {"a": 1},
    }));
    let source = object(json!({
        "list": [2, 3],
        "fallbackModel": ["b"],
        "managedMcpServers": {"one": {"new": true}},
        "modelPicker": {"new": 2},
        "__proto__": {"b": 1},
        "scalar": {"__proto__": 1},
    }));
    let mut warmth = MergeWarmth::WARM;
    assert_eq!(
        merge_within(&rules(), &mut destination, &source, room(10), &mut warmth),
        NodeMerge::Merged
    );
    assert_eq!(
        Value::Object(destination),
        json!({
            "list": [1, 2, 3],
            "fallbackModel": ["b"],
            "managedMcpServers": {"one": {"new": true}},
            "modelPicker": {"new": 2},
            "__proto__": {"a": 1},
            "scalar": {"__proto__": 1},
        })
    );
}

#[test]
fn a_merge_on_a_stack_with_no_room_writes_nothing() {
    let mut destination = Map::new();
    let source = object(json!({"a": 1}));
    let mut warmth = MergeWarmth::WARM;
    assert_eq!(
        merge_within(&rules(), &mut destination, &source, room(0), &mut warmth),
        NodeMerge::Overflowed
    );
    assert!(destination.is_empty());
}

/// The bytes a merge needs for a root object holding `value` under `x`: the most it spends at any point.
fn needs(warmth: MergeWarmth, value: Value) -> usize {
    let source = Value::Object(Map::from_iter([("x".to_owned(), value)]));
    let fits = |available: usize| merged(warmth, available, &mut Map::new(), &source) == NodeMerge::Merged;
    let (mut low, mut high) = (0, 4 * COST.level + COST.shallow_array);
    while low < high {
        let middle = (low + high) / 2;
        if fits(middle) {
            high = middle;
        } else {
            low = middle + 1;
        }
    }
    low
}

#[test]
fn a_merge_spends_more_where_v8_runs_part_of_lodash_for_the_first_time_or_unoptimized() {
    let cold = MergeWarmth::default();
    let assigned = MergeWarmth {
        assigned: true,
        ..MergeWarmth::default()
    };
    let two = 2 * COST.level;
    assert_eq!(
        [
            needs(cold, json!(null)),
            needs(MergeWarmth::WARM, json!(null)),
            needs(cold, json!({})),
            needs(cold, json!([])),
            needs(MergeWarmth::WARM, json!([])),
            needs(cold, json!([null])),
            needs(MergeWarmth::WARM, json!([{}])),
            needs(assigned, json!([[[[]]]])),
            needs(MergeWarmth::WARM, json!([[[[]]]])),
        ],
        [
            COST.level + COST.cold_assign,
            COST.level,
            two,
            two + COST.first_array,
            two,
            two + COST.shallow_array,
            two + COST.shallow_array,
            5 * COST.level,
            5 * COST.level,
        ]
    );
    assert_eq!(needs(assigned, json!([[[]]])), 4 * COST.level + COST.first_array);
}

#[test]
fn a_member_assigned_or_an_array_finished_before_warms_the_rest_of_the_merge() {
    let cold = MergeWarmth::default();
    let deep = |levels: usize| (1..levels).fold(json!(null), |value, _| json!({"x": value}));
    let mut warmed = Map::new();
    let source = json!({"a": 1, "x": deep(10)});
    let available = 10 * COST.level + COST.cold_assign - 1;
    assert_eq!(merged(cold, available, &mut warmed, &source), NodeMerge::Merged);
    let mut cold_first = Map::new();
    let source = json!({"x": deep(10), "a": 1});
    assert_eq!(merged(cold, available, &mut cold_first, &source), NodeMerge::Overflowed);
    let mut after_array = Map::new();
    let source = json!({"a": [], "x": {"y": []}});
    assert_eq!(
        merged(cold, 3 * COST.level + COST.cold_assign, &mut after_array, &source),
        NodeMerge::Merged
    );
    let mut first_array = Map::new();
    let source = json!({"x": {"y": []}});
    assert_eq!(
        merged(cold, 3 * COST.level + COST.cold_assign, &mut first_array, &source),
        NodeMerge::Overflowed
    );
}

#[test]
fn only_the_first_two_index_tests_of_the_process_compile_wherever_their_array_sits() {
    let two = 2 * COST.level;
    let compiled = MergeWarmth {
        index_tests: MergeWarmth::INDEX_COMPILES,
        ..MergeWarmth::WARM
    };
    assert_eq!(needs(compiled, json!([null])), two);
    let mut warmth = MergeWarmth::WARM;
    let source = object(json!({"a": [], "b": [1], "c": [[2]], "d": [3]}));
    assert_eq!(
        merge_within(&rules(), &mut Map::new(), &source, room(10), &mut warmth),
        NodeMerge::Merged
    );
    assert_eq!(warmth.index_tests, MergeWarmth::INDEX_COMPILES);
    let mut warmth = MergeWarmth::WARM;
    let mut destination = object(json!({"b": [0], "c": {"length": 1}}));
    let source = object(json!({"b": [1], "c": [2]}));
    assert_eq!(
        merge_within(&rules(), &mut destination, &source, room(10), &mut warmth),
        NodeMerge::Merged
    );
    assert_eq!(warmth.index_tests, 1);
    let mut warmth = MergeWarmth::WARM;
    let source = object(json!({"a": [1, 1]}));
    assert_eq!(
        merge_within(&rules(), &mut Map::new(), &source, room(10), &mut warmth),
        NodeMerge::Merged
    );
    assert_eq!(warmth.index_tests, MergeWarmth::INDEX_COMPILES);
}

#[test]
fn an_array_that_runs_out_of_stack_counts_as_not_entered() {
    let mut warmth = MergeWarmth::WARM;
    let source = object(json!({"a": [1]}));
    assert_eq!(
        merge_within(&rules(), &mut Map::new(), &source, room(2), &mut warmth),
        NodeMerge::Overflowed
    );
    assert_eq!(warmth.index_tests, 0);
}

#[test]
fn an_array_that_compiles_the_index_pattern_without_the_stack_for_it_rejects_with_a_syntax_error() {
    let cold = MergeWarmth::default();
    let once = MergeWarmth {
        index_tests: 1,
        ..MergeWarmth::default()
    };
    let source = json!({"x": [null]});
    let two = 2 * COST.level;
    assert_eq!(
        [
            merged(cold, two + COST.first_array, &mut Map::new(), &source),
            merged(cold, two + COST.first_array - 1, &mut Map::new(), &source),
            merged(once, COST.level, &mut Map::new(), &source),
        ],
        [
            NodeMerge::InvalidRegularExpression,
            NodeMerge::Overflowed,
            NodeMerge::InvalidRegularExpression,
        ]
    );
}

#[test]
fn lodash_enters_levels_past_the_stack_within_its_descent_reach_and_runs_out_as_it_returns() {
    let once = MergeWarmth {
        index_tests: 1,
        ..MergeWarmth::WARM
    };
    let deepest = (COST.level + COST.descent_reach) / COST.level;
    let source = |objects: usize, leaf: Value| json!({"x": (0..objects).fold(leaf, |value, _| json!({"x": value}))});
    assert_eq!(
        [
            merged(once, COST.level, &mut Map::new(), &source(deepest - 2, json!([1]))),
            merged(once, COST.level, &mut Map::new(), &source(deepest - 1, json!([1]))),
            merged(once, COST.level, &mut Map::new(), &source(deepest - 2, json!({}))),
        ],
        [
            NodeMerge::InvalidRegularExpression,
            NodeMerge::Overflowed,
            NodeMerge::Overflowed,
        ]
    );
    let mut destination = object(json!({"x": {"z": 0}}));
    assert_eq!(
        merged(once, COST.level, &mut destination, &json!({"x": {"y": [1]}})),
        NodeMerge::InvalidRegularExpression
    );
    assert_eq!(Value::Object(destination), json!({"x": {"z": 0}}));
}

#[test]
fn the_machine_code_compile_of_the_index_pattern_spends_more_in_a_merge_after_the_one_that_compiled_it_to_bytecode() {
    let later = MergeCost {
        later_compile: COST.level,
        ..COST
    };
    let available = 4 * COST.level;
    let objects = (available + COST.descent_reach) / COST.level - 2;
    let deep = (0..objects).fold(json!([1]), |value, _| json!({"x": value}));
    let merge = |cost: MergeCost, warmth: MergeWarmth, source: Value| {
        let mut warmth = warmth;
        let room = MergeRoom { cost, available };
        merge_within(&rules(), &mut Map::new(), &object(source), room, &mut warmth)
    };
    let once = MergeWarmth {
        index_tests: 1,
        ..MergeWarmth::WARM
    };
    assert_eq!(
        [
            merge(COST, once, json!({"x": deep})),
            merge(later, once, json!({"x": deep})),
            merge(later, MergeWarmth::WARM, json!({"a": [1], "x": deep})),
        ],
        [
            NodeMerge::InvalidRegularExpression,
            NodeMerge::Overflowed,
            NodeMerge::InvalidRegularExpression,
        ]
    );
}

/// A merge that has tested both array indexes that compile the `isIndex` pattern, assigned a member and finished an
/// array: every level spends only the level.
const HOT: MergeWarmth = MergeWarmth {
    index_tests: MergeWarmth::INDEX_COMPILES,
    ..MergeWarmth::WARM
};

#[test]
fn a_held_object_entered_past_the_stack_runs_out_as_the_merge_returns_from_it() {
    let mut destination = object(json!({"x": {"z": 0}}));
    assert_eq!(
        merged(HOT, COST.level, &mut destination, &json!({"x": {}})),
        NodeMerge::Overflowed
    );
    assert_eq!(Value::Object(destination), json!({"x": {"z": 0}}));
}

#[test]
fn an_array_merged_onto_a_copy_of_an_array_like_object_that_runs_out_writes_no_copy() {
    let mut destination = object(json!({"x": {"length": 1}}));
    assert_eq!(
        merged(HOT, 2 * COST.level, &mut destination, &json!({"x": [[1]]})),
        NodeMerge::Overflowed
    );
    assert_eq!(Value::Object(destination), json!({"x": {"length": 1}}));
}

/// Outside a resolve the merge marks the holes of its arrays with [`ArrayMark::FALLBACK`]. The source holds an array
/// of more holes than a V8 string holds commas, as an earlier merge leaves where an object sets the `length` of an
/// array.
#[test]
fn a_length_set_to_an_array_that_joins_past_a_v8_string_rejects_with_an_invalid_string_length() {
    let holes = Value::Array(vec![ArrayMark::FALLBACK.run(V8_MAX_STRING_LENGTH + 2)]);
    let source = Value::Object(Map::from_iter([(
        "o".to_owned(),
        Value::Object(Map::from_iter([("length".to_owned(), holes)])),
    )]));
    let ended = merged(HOT, 10 * COST.level, &mut object(json!({"o": []})), &source);
    assert_eq!(ended, NodeMerge::InvalidStringLength);
    assert!(matches!(
        ended.rejection(),
        Some(ResolveSettingsError::InvalidStringLength)
    ));
}
