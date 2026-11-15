//! How deep the Node process of the TypeScript SDK walks a settings document before V8 runs out of stack, which sets
//! the deep settings files `resolveSettings` reads, skips and rejects.
//!
//! `JSON.parse` reads a document of any depth, but the settings file reader (`lE` upstream) then copies it with
//! `structuredClone` and checks the copy against the settings schema, both recursively. When either one runs out of
//! stack, the reader skips the file as one it could not read (`Q7e`), as it skips invalid JSON. The file cache (`Fu`)
//! then hands out another `structuredClone` of what the reader kept, outside that guard, so running out of stack there
//! rejects `resolveSettings` with `RangeError: Maximum call stack size exceeded`. So does lodash `mergeWith`, which
//! `resolveSettings` merges each source into the result with, one call per array or object it enters.
//!
//! Each pass spends a fixed number of bytes on each object and each array of a path from the root, objects and
//! arrays apart, and another at the array or object that ends the path, by whether it is empty. The merge spends a
//! fixed number of bytes on each level, and more where V8 runs part of lodash for the first time in the process or
//! runs it unoptimized, so the merges of one resolve share what ran before them. On macOS the stack each pass has is
//! what V8 reserves less what the command line and environment fill when the process starts; on Linux V8 sets its
//! limit below them, so they take nothing from it. V8's stack size and frame sizes set every number, so each profile
//! names the Node release and platform it was measured on, and [`NodeStack::for_build_target`] picks the one for the
//! platform the crate is built for.
//!
//! V8 compiles lodash on background threads while a merge runs and installs the code at times that vary from one run
//! of the same process to the next, which moves where a deep merge runs out: when most levels of a path assign a
//! string, number, boolean or `null`, the optimized merge reaches deeper levels, and on linux x64 a merge after the one
//! that compiled the `isIndex` pattern can run out one level shallower compiling it to machine code. Each profile
//! assumes no background compile lands while a merge runs, as on Node with every function kept in the interpreter.
//!
//! A managed settings document (`lE(..., true)` upstream) goes through the same passes on a deeper stack. When its
//! reader runs out, the document does not load. The file cache copy and the merge into its folder (`UP`) run outside
//! that guard: for the managed settings file they reject `resolveSettings`, and for a drop-in they end the folder
//! walk inside its `try`, which keeps what merged before and marks the folder as one that did not load. The admin
//! settings are then merged once more onto a fresh object (`QP`), which rejects when it runs out.
//!
//! `resolveSettings` evaluates the managed settings up to three times: for the sources and the effective settings,
//! for the policy origin, and to ask whether the machine policy loaded. The file cache keeps each document's parse
//! from the first evaluation that reaches it, but each evaluation copies and merges on its own, deeper, stack.

use std::cell::Cell;

use serde_json::{Map, Value};

use crate::{
    node_managed_pass::ManagedPass,
    node_merge::{NodeMerge, merge_within},
    node_merge_call::NodeMergeCall,
    node_merge_cost::MergeCost,
    node_merge_room::MergeRoom,
    node_merge_warmth::MergeWarmth,
    settings_merge_spec::MergeSpec,
};

/// The stack bytes one pass over a settings document spends on each object and each array of a path, and how much
/// it has before the command line and environment take their share.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PathCost {
    object: usize,
    array: usize,
    leaves: LeafCost,
    budget: usize,
}

/// What a pass spends at an object or array that holds no arrays or objects, in place of what it spends at one that
/// does, by whether it is empty or holds strings, numbers, booleans or `null`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct LeafCost {
    empty_object: usize,
    scalar_object: usize,
    empty_array: usize,
    scalar_array: usize,
}

impl PathCost {
    /// What `value` spends while the pass is at it.
    fn at(self, value: &Value) -> usize {
        match value {
            Value::Object(entries) => self.at_object(entries),
            Value::Array(items) => self.at_array(items),
            _ => 0,
        }
    }

    fn at_object(self, entries: &Map<String, Value>) -> usize {
        if entries.values().any(is_container) {
            self.object
        } else if entries.is_empty() {
            self.leaves.empty_object
        } else {
            self.leaves.scalar_object
        }
    }

    fn at_array(self, items: &[Value]) -> usize {
        if items.iter().any(is_container) {
            self.array
        } else if items.is_empty() {
            self.leaves.empty_array
        } else {
            self.leaves.scalar_array
        }
    }

    /// What `value` leaves spent for the arrays and objects it holds.
    fn under(self, value: &Value) -> usize {
        match value {
            Value::Object(_) => self.object,
            Value::Array(_) => self.array,
            _ => 0,
        }
    }

    /// Whether every path from the root of `document` fits in `available` bytes.
    fn fits(self, available: usize, document: &Value) -> bool {
        self.fits_below(available, vec![(document, 0)])
    }

    /// Whether every path from a root object holding `entries` fits in `available` bytes.
    fn fits_object(self, available: usize, entries: &Map<String, Value>) -> bool {
        self.at_object(entries) <= available
            && self.fits_below(
                available,
                entries
                    .values()
                    .filter(|entry| is_container(entry))
                    .map(|entry| (entry, self.object))
                    .collect(),
            )
    }

    /// Whether every path from each of `pending`, after the bytes spent above it, fits in `available` bytes.
    fn fits_below(self, available: usize, mut pending: Vec<(&Value, usize)>) -> bool {
        while let Some((value, spent)) = pending.pop() {
            if spent.saturating_add(self.at(value)) > available {
                return false;
            }
            let spent = spent.saturating_add(self.under(value));
            let children: Box<dyn Iterator<Item = &Value>> = match value {
                Value::Array(items) => Box::new(items.iter()),
                Value::Object(entries) => Box::new(entries.values()),
                _ => continue,
            };
            pending.extend(children.filter(|child| is_container(child)).map(|child| (child, spent)));
        }
        true
    }

    /// The most arrays and objects a path that fits in `available` bytes may hold.
    fn deepest(self, available: usize) -> usize {
        available / self.object.min(self.array).max(1)
    }
}

/// The stack the TypeScript SDK's Node process has for reading and merging settings files, for one Node release
/// and platform.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NodeStack {
    startup: StartupShare,
    startup_bytes: usize,
    walk: PathCost,
    clone: PathCost,
    copy: PathCost,
    merge: MergeCost,
    merge_budget: usize,
    policy: PolicyBudgets,
}

/// Whether what the command line and environment fill when Node starts comes off the stack its passes have.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StartupShare {
    /// macOS, where V8's limit sits a fixed distance below the top of the main thread's stack, above the command line
    /// and environment.
    FromStack,
    /// Linux, where V8 sets its limit from the stack pointer it starts with, below the command line and environment.
    BesideStack,
}

/// The budgets a managed settings document's passes have, out of the same stack, in each evaluation of the managed
/// settings that reads it, and the budget of the merge of the admin settings.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PolicyBudgets {
    sources: PassBudgets,
    origin: PassBudgets,
    machine_policy: PassBudgets,
    admin_merge: usize,
}

/// The budgets of the reader's walk and clone, the file cache copy and the merge into the folder in one evaluation of
/// the managed settings.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PassBudgets {
    walk: usize,
    clone: usize,
    copy: usize,
    folder_merge: usize,
}

impl PolicyBudgets {
    fn of(self, pass: ManagedPass) -> PassBudgets {
        match pass {
            ManagedPass::Sources => self.sources,
            ManagedPass::Origin => self.origin,
            ManagedPass::MachinePolicy => self.machine_policy,
        }
    }
}

impl NodeStack {
    /// Node 24.13.1 on macOS arm64 with its default stack, started with a command line and environment of 4384 bytes.
    ///
    /// The reader reads a settings file when every path from its root fits two passes: 448 bytes per object and 240
    /// per array out of 1000451, and 288 per object and 304 per array out of 1000724. The file cache copy then takes
    /// 288 per object and 512 per array out of 1000884. At the object or array that ends a path, the walk takes 144
    /// for an empty object, 176 for an empty array, 448 for an object and 272 for an array holding a scalar; the clone
    /// takes 328 for any object and 304 for any array; and the copy takes 496 for an empty object, 528 for an object
    /// holding a scalar and 512 for any array.
    ///
    /// A merge takes 768 per array or object it enters out of 960611. Assigning a member takes 416 more while no merge
    /// of the process assigned one before. An array takes 1339 more when it holds members and lodash `isIndex`
    /// tested fewer than two keys in the merges of the process before, one per member of each array they entered,
    /// wherever it sits, and else 576 more when it is one of the first three of its path while no merge finished an
    /// array before. lodash still enters an array or object up to 40230 bytes past that, merges its members and runs
    /// out as it returns from it, and an array holding members there that compiles the `isIndex` regular expression
    /// rejects with `SyntaxError`: the first compile where the level and 576 more fit, the second anywhere in reach.
    /// At this startup that is 2223 nested objects or a root object holding 3276 nested arrays for the reader, 1945
    /// nested arrays for the copy, and 1244 levels around `null` for a merge.
    ///
    /// A managed settings document's reader has 997859 and 998160 for the same two passes when the sources are
    /// evaluated, its copy 998787, the merge into its folder 958147 and the merge of the admin settings 960371: 2217
    /// nested objects or a root object holding 3268 nested arrays, 1941 nested arrays, 1241 levels and 1244 levels.
    /// The evaluation of the policy origin has 998944, 999245, 999488 and 959008 (2220 objects, 1943 arrays, 1243
    /// levels), and the one that asks whether the machine policy loaded 999840, 1000141, 1000576 and 959776 (2222
    /// objects, 1945 arrays, 1244 levels). These were measured with a user settings file read first, as the settings
    /// goldens do.
    pub const NODE_24_DARWIN_ARM64: Self = Self {
        startup: StartupShare::FromStack,
        startup_bytes: 4384,
        walk: PathCost {
            object: 448,
            array: 240,
            leaves: LeafCost {
                empty_object: 144,
                scalar_object: 448,
                empty_array: 176,
                scalar_array: 272,
            },
            budget: 1_000_451,
        },
        clone: PathCost {
            object: 288,
            array: 304,
            leaves: LeafCost {
                empty_object: 328,
                scalar_object: 328,
                empty_array: 304,
                scalar_array: 304,
            },
            budget: 1_000_724,
        },
        copy: PathCost {
            object: 288,
            array: 512,
            leaves: LeafCost {
                empty_object: 496,
                scalar_object: 528,
                empty_array: 512,
                scalar_array: 512,
            },
            budget: 1_000_884,
        },
        merge: MergeCost {
            level: 768,
            cold_assign: 416,
            first_array: 576,
            shallow_array: 1339,
            descent_reach: 40_230,
            later_compile: 0,
        },
        merge_budget: 960_611,
        policy: PolicyBudgets {
            sources: PassBudgets {
                walk: 997_859,
                clone: 998_160,
                copy: 998_787,
                folder_merge: 958_147,
            },
            origin: PassBudgets {
                walk: 998_944,
                clone: 999_245,
                copy: 999_488,
                folder_merge: 959_008,
            },
            machine_policy: PassBudgets {
                walk: 999_840,
                clone: 1_000_141,
                copy: 1_000_576,
                folder_merge: 959_776,
            },
            admin_merge: 960_371,
        },
    };

    /// Node 24.13.1 on Linux arm64 with its default stack, the `node:24.13.1-bookworm` image.
    ///
    /// The reader reads a settings file when every path from its root fits two passes: 512 bytes per object and 272
    /// per array out of 1002224, and 320 per object and per array out of 1002392. The file cache copy then takes 512
    /// per object and 560 per array out of 1002208. At the object or array that ends a path, the walk and the copy
    /// take 144 for an empty object, 192 for an empty array and what they take per object or array for one holding a
    /// scalar; the clone takes 320 for any of them.
    ///
    /// A merge takes 768 per array or object it enters out of 962512, with the same extra costs and reach as
    /// [`NodeStack::NODE_24_DARWIN_ARM64`]: 416, 576, 1339 and 40230. That is 1957 nested objects or a root object
    /// holding 3131 nested arrays for the reader, 1788 nested arrays for the copy, and 1252 levels around `null` for a
    /// merge.
    ///
    /// A managed settings document's reader has 999552 and 999840 for the same two passes when the sources are
    /// evaluated, its copy 999824, the merge into its folder 959824 and the merge of the admin settings 961920: 1952
    /// nested objects or a root object holding 3123 nested arrays, 1784 nested arrays, 1249 levels and 1251 levels.
    /// The evaluation of the policy origin has 1000704, 1001120, 1000952 and 961152 (1954 objects, 1250 levels), and
    /// the one that asks whether the machine policy loaded 1001728, 1002080, 1002064 and 961824 (1956 objects, 1788
    /// arrays, 1252 levels). No golden tells the origin's copy budget apart from the others, so it sits between them.
    ///
    /// The command line and environment take nothing from these budgets, so [`NodeStack::with_startup_bytes`] changes
    /// nothing here; the goldens were recorded with the same 4384 bytes as on macOS.
    pub const NODE_24_LINUX_ARM64: Self = Self {
        startup: StartupShare::BesideStack,
        startup_bytes: 4384,
        walk: PathCost {
            object: 512,
            array: 272,
            leaves: LeafCost {
                empty_object: 144,
                scalar_object: 512,
                empty_array: 192,
                scalar_array: 272,
            },
            budget: 1_002_224,
        },
        clone: PathCost {
            object: 320,
            array: 320,
            leaves: LeafCost {
                empty_object: 320,
                scalar_object: 320,
                empty_array: 320,
                scalar_array: 320,
            },
            budget: 1_002_392,
        },
        copy: PathCost {
            object: 512,
            array: 560,
            leaves: LeafCost {
                empty_object: 144,
                scalar_object: 512,
                empty_array: 192,
                scalar_array: 560,
            },
            budget: 1_002_208,
        },
        merge: MergeCost {
            level: 768,
            cold_assign: 416,
            first_array: 576,
            shallow_array: 1339,
            descent_reach: 40_230,
            later_compile: 0,
        },
        merge_budget: 962_512,
        policy: PolicyBudgets {
            sources: PassBudgets {
                walk: 999_552,
                clone: 999_840,
                copy: 999_824,
                folder_merge: 959_824,
            },
            origin: PassBudgets {
                walk: 1_000_704,
                clone: 1_001_120,
                copy: 1_000_952,
                folder_merge: 961_152,
            },
            machine_policy: PassBudgets {
                walk: 1_001_728,
                clone: 1_002_080,
                copy: 1_002_064,
                folder_merge: 961_824,
            },
            admin_merge: 961_920,
        },
    };

    /// Node 24.13.1 on Linux x86-64 with its default stack, the `node:24.13.1-bookworm` image.
    ///
    /// The reader reads a settings file when every path from its root fits two passes: 608 bytes per object and 272
    /// per array out of 1002688, and 304 per object and per array out of 1002760. The file cache copy then takes 608
    /// per object and 640 per array out of 1002496. At the object or array that ends a path, the walk and the copy
    /// take 144 for an empty object, 192 for an empty array and what they take per object or array for one holding a
    /// scalar; the clone takes 304 for any of them.
    ///
    /// A merge takes 736 per array or object it enters out of 962848, with extra costs of 416, 576 and 1339 as on
    /// the other profiles and a reach of 40656. That is 1649 nested objects or a root object holding 3297 nested
    /// arrays for the reader, 1565 nested arrays for the copy, and 1307 levels around `null` for a merge.
    ///
    /// A managed settings document's reader has 999992 and 1000312 for the same two passes when the sources are
    /// evaluated, its copy 1000608, the merge into its folder 960320 and the merge of the admin settings 962320: 1644
    /// nested objects or a root object holding 3289 nested arrays, 1562 nested arrays, 1304 levels and 1306 levels.
    /// The evaluation of the policy origin has 1001072, 1001528, 1001688 and 961584 (1646 objects, 1305 levels), and
    /// the one that asks whether the machine policy loaded 1002288, 1002440, 1002528 and 962256 (1648 objects, 1565
    /// arrays, 1307 levels). No golden tells the origin's copy budget apart from the others, so it sits between them.
    ///
    /// When a background compile lands while a merge runs, a merge after the one that compiled the `isIndex` pattern
    /// to bytecode can run out at 1362 levels instead of compiling the pattern there, and any `later_compile` from 337
    /// to 1072 moves that boundary alone. The profile assumes no background compile lands, as the others do, so it
    /// spends nothing there. The command line and environment take nothing from these budgets, so
    /// [`NodeStack::with_startup_bytes`] changes nothing here.
    pub const NODE_24_LINUX_X64: Self = Self {
        startup: StartupShare::BesideStack,
        startup_bytes: 4384,
        walk: PathCost {
            object: 608,
            array: 272,
            leaves: LeafCost {
                empty_object: 144,
                scalar_object: 608,
                empty_array: 192,
                scalar_array: 272,
            },
            budget: 1_002_688,
        },
        clone: PathCost {
            object: 304,
            array: 304,
            leaves: LeafCost {
                empty_object: 304,
                scalar_object: 304,
                empty_array: 304,
                scalar_array: 304,
            },
            budget: 1_002_760,
        },
        copy: PathCost {
            object: 608,
            array: 640,
            leaves: LeafCost {
                empty_object: 144,
                scalar_object: 608,
                empty_array: 192,
                scalar_array: 640,
            },
            budget: 1_002_496,
        },
        merge: MergeCost {
            level: 736,
            cold_assign: 416,
            first_array: 576,
            shallow_array: 1339,
            descent_reach: 40_656,
            later_compile: 0,
        },
        merge_budget: 962_848,
        policy: PolicyBudgets {
            sources: PassBudgets {
                walk: 999_992,
                clone: 1_000_312,
                copy: 1_000_608,
                folder_merge: 960_320,
            },
            origin: PassBudgets {
                walk: 1_001_072,
                clone: 1_001_528,
                copy: 1_001_688,
                folder_merge: 961_584,
            },
            machine_policy: PassBudgets {
                walk: 1_002_288,
                clone: 1_002_440,
                copy: 1_002_528,
                folder_merge: 962_256,
            },
            admin_merge: 962_320,
        },
    };

    /// The profile for the platform the crate is built for: [`NodeStack::NODE_24_LINUX_ARM64`] on Linux arm64,
    /// [`NodeStack::NODE_24_LINUX_X64`] on Linux x86-64 and [`NodeStack::NODE_24_DARWIN_ARM64`] everywhere else.
    #[must_use]
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    pub const fn for_build_target() -> Self {
        Self::NODE_24_LINUX_ARM64
    }

    /// The profile for the platform the crate is built for: [`NodeStack::NODE_24_LINUX_ARM64`] on Linux arm64,
    /// [`NodeStack::NODE_24_LINUX_X64`] on Linux x86-64 and [`NodeStack::NODE_24_DARWIN_ARM64`] everywhere else.
    #[must_use]
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    pub const fn for_build_target() -> Self {
        Self::NODE_24_LINUX_X64
    }

    /// The profile for the platform the crate is built for: [`NodeStack::NODE_24_LINUX_ARM64`] on Linux arm64,
    /// [`NodeStack::NODE_24_LINUX_X64`] on Linux x86-64 and [`NodeStack::NODE_24_DARWIN_ARM64`] everywhere else.
    #[must_use]
    #[cfg(not(all(target_os = "linux", any(target_arch = "aarch64", target_arch = "x86_64"))))]
    pub const fn for_build_target() -> Self {
        Self::NODE_24_DARWIN_ARM64
    }

    /// The same stack for a process whose command line and environment fill `bytes` when it starts: each argument
    /// and each `NAME=value` entry takes its UTF-8 length plus 9 bytes, and the path of the executable takes as much
    /// once more, as `executable_path=PATH`. Only the macOS profile spends them out of its budgets.
    #[must_use]
    pub const fn with_startup_bytes(self, bytes: usize) -> Self {
        Self {
            startup_bytes: bytes,
            ..self
        }
    }

    fn available(self, budget: usize) -> usize {
        match self.startup {
            StartupShare::FromStack => budget.saturating_sub(self.startup_bytes),
            StartupShare::BesideStack => budget,
        }
    }

    /// The most nested arrays and objects a settings file the reader reads may hold.
    pub(crate) fn max_depth(self) -> usize {
        self.walk
            .deepest(self.available(self.walk.budget))
            .min(self.clone.deepest(self.available(self.clone.budget)))
    }

    /// The most nested arrays and objects a managed settings document the reader reads in `pass` may hold.
    pub(crate) fn policy_max_depth(self, pass: ManagedPass) -> usize {
        let budgets = self.policy.of(pass);
        self.walk
            .deepest(self.available(budgets.walk))
            .min(self.clone.deepest(self.available(budgets.clone)))
    }

    /// Whether the settings file reader walks and copies `document` without running out of stack.
    pub(crate) fn reads(self, document: &Value) -> bool {
        self.walk.fits(self.available(self.walk.budget), document)
            && self.clone.fits(self.available(self.clone.budget), document)
    }

    /// Whether the managed settings reader walks and copies `document` in `pass` without running out of stack.
    pub(crate) fn reads_policy(self, pass: ManagedPass, document: &Map<String, Value>) -> bool {
        let budgets = self.policy.of(pass);
        self.walk.fits_object(self.available(budgets.walk), document)
            && self.clone.fits_object(self.available(budgets.clone), document)
    }

    /// Whether the file cache runs out of stack copying `settings`, which the reader kept from a settings file.
    pub(crate) fn copy_overflows(self, settings: &Map<String, Value>) -> bool {
        !self.copy.fits_object(self.available(self.copy.budget), settings)
    }

    /// Whether the file cache runs out of stack in `pass` copying `settings`, which the reader kept from a managed
    /// settings document.
    pub(crate) fn policy_copy_overflows(self, pass: ManagedPass, settings: &Map<String, Value>) -> bool {
        !self
            .copy
            .fits_object(self.available(self.policy.of(pass).copy), settings)
    }

    /// Merges `source` into `destination` in `call` as lodash `mergeWith` with the settings customizer does, keeping
    /// what it wrote before it runs out of stack. `warmth` is what the merges of the resolve that ran before this one
    /// left V8 having run, in the order the TypeScript SDK runs them: the merges into the folder and of the admin
    /// settings in the evaluation of the sources, the merges of the sources, then the merges into the folder in the
    /// evaluations of the policy origin and of the machine policy. This merge adds what it ran.
    pub(crate) fn merge_within(
        self,
        call: NodeMergeCall,
        rules: &MergeSpec,
        destination: &mut Map<String, Value>,
        source: &Map<String, Value>,
        warmth: &Cell<MergeWarmth>,
    ) -> NodeMerge {
        let budget = match call {
            NodeMergeCall::Source => self.merge_budget,
            NodeMergeCall::Folder(pass) => self.policy.of(pass).folder_merge,
            NodeMergeCall::Admin => self.policy.admin_merge,
        };
        let room = MergeRoom {
            cost: self.merge,
            available: self.available(budget),
        };
        let mut ran = warmth.get();
        let merged = merge_within(rules, destination, source, room, &mut ran);
        warmth.set(ran);
        merged
    }

    /// Merges `source` into `destination` as [`Self::merge_within`] does with room for any depth, where no Node stack
    /// bounds the merge: it stops only where lodash throws on every stack.
    pub(crate) fn merge_unbounded(
        self,
        rules: &MergeSpec,
        destination: &mut Map<String, Value>,
        source: &Map<String, Value>,
    ) -> NodeMerge {
        let room = MergeRoom {
            cost: self.merge,
            available: usize::MAX,
        };
        merge_within(rules, destination, source, room, &mut MergeWarmth::default())
    }
}

impl Default for NodeStack {
    fn default() -> Self {
        Self::for_build_target()
    }
}

fn is_container(value: &Value) -> bool {
    value.is_object() || value.is_array()
}

#[cfg(test)]
#[path = "node_stack_tests.rs"]
mod tests;
