use std::collections::{HashMap, HashSet};
use std::hash::Hash;

use clippy_utils::diagnostics::span_lint_hir_and_then;
use clippy_utils::is_in_test;
use rustc_hir::def::{DefKind, Res};
use rustc_hir::def_id::{CRATE_DEF_INDEX, DefId};
use rustc_hir::{AmbigArg, Expr, ExprKind, HirId, Item, ItemKind, Ty, TyKind};
use rustc_lint::LateContext;
use rustc_middle::ty::TyCtxt;
use rustc_span::Span;

use crate::ACYCLIC_MODULES;
use crate::test_context::{is_test_module_name, is_test_or_bench_source};

/// One resolved reference from an item in `source`'s subtree to an item defined
/// in `target`'s subtree. Both are module `DefId`s, kept whole rather than
/// collapsed to a top-level name, so the sibling pair a cycle is reported
/// against can be recovered at every depth.
struct ModuleReference {
    source: DefId,
    target: DefId,
    span: Span,
}

/// A directed edge between two direct children of the same parent module, with
/// the reference that first witnessed it.
struct SiblingEdge {
    from: DefId,
    to: DefId,
    span: Span,
}

#[derive(Default)]
pub(crate) struct AcyclicModules {
    references: Vec<ModuleReference>,
}

impl AcyclicModules {
    pub(crate) fn check_expr<'tcx>(&mut self, cx: &LateContext<'tcx>, expr: &'tcx Expr<'tcx>) {
        let qpath = match expr.kind {
            ExprKind::Path(ref qpath) => Some(qpath),
            ExprKind::Struct(qpath, _, _) => Some(qpath),
            _ => None,
        };

        let referenced = match qpath {
            Some(qpath) => match cx.qpath_res(qpath, expr.hir_id) {
                Res::Def(_, def_id) => Some(def_id),
                _ => None,
            },
            None if matches!(expr.kind, ExprKind::MethodCall(..)) => {
                cx.typeck_results().type_dependent_def_id(expr.hir_id)
            }
            None => None,
        };

        if let Some(def_id) = referenced {
            self.record(cx, def_id, expr.hir_id, expr.span);
        }
    }

    pub(crate) fn check_ty<'tcx>(&mut self, cx: &LateContext<'tcx>, ty: &'tcx Ty<'tcx, AmbigArg>) {
        if let TyKind::Path(ref qpath) = ty.kind
            && let Res::Def(_, def_id) = cx.qpath_res(qpath, ty.hir_id)
        {
            self.record(cx, def_id, ty.hir_id, ty.span);
        }
    }

    pub(crate) fn check_item<'tcx>(&mut self, cx: &LateContext<'tcx>, item: &'tcx Item<'tcx>) {
        let ItemKind::Use(path, _) = item.kind else {
            return;
        };

        for res in path.res.present_items() {
            if let Res::Def(_, def_id) = res {
                self.record(cx, def_id, item.hir_id(), item.span);
            }
        }
    }

    pub(crate) fn check_crate_post(&mut self, cx: &LateContext<'_>) {
        let graphs = build_sibling_graphs(cx.tcx, &self.references);

        let mut parents: Vec<DefId> = graphs.keys().copied().collect();
        parents.sort_by_cached_key(|parent| module_path(cx.tcx, *parent));

        for parent in parents {
            let Some(edges) = graphs.get(&parent) else {
                continue;
            };
            report_cycles(cx, parent, edges);
        }
    }

    /// Records the module-to-module edge a reference implies, unless the
    /// reference cannot express an architectural dependency: items from another
    /// crate (Cargo already forbids cycles there), macro expansions (the path is
    /// the macro author's, not this call site's), and test code.
    fn record(&mut self, cx: &LateContext<'_>, def_id: DefId, hir_id: HirId, span: Span) {
        if !def_id.is_local() || span.from_expansion() {
            return;
        }

        // Most references in a crate stay inside their own module, so this is
        // the check that keeps the rest of the work off the common path.
        let source = cx.tcx.parent_module(hir_id).to_def_id();
        let target = enclosing_module(cx.tcx, def_id);
        if source == target {
            return;
        }

        if is_test_module_subtree(cx.tcx, source)
            || is_test_module_subtree(cx.tcx, target)
            || is_test_context(cx, hir_id, span)
        {
            return;
        }

        self.references.push(ModuleReference {
            source,
            target,
            span,
        });
    }
}

/// Groups every recorded reference into the sibling graph of the deepest module
/// that contains both of its endpoints.
///
/// The two endpoints' ancestor chains share a prefix; the first position where
/// they diverge names the two siblings, and the module just above it owns the
/// graph. A reference whose endpoints are ancestor and descendant never
/// diverges, so parent-child edges (a parent declaring or re-exporting its
/// child, a child reaching up through `super::`) are excluded by construction
/// rather than by a special case.
fn build_sibling_graphs(
    tcx: TyCtxt<'_>,
    references: &[ModuleReference],
) -> HashMap<DefId, Vec<SiblingEdge>> {
    let mut graphs: HashMap<DefId, Vec<SiblingEdge>> = HashMap::new();

    for reference in references {
        let source_chain = module_chain(tcx, reference.source);
        let target_chain = module_chain(tcx, reference.target);

        let shared = source_chain
            .iter()
            .zip(target_chain.iter())
            .take_while(|(source, target)| source == target)
            .count();

        if shared == 0 || shared >= source_chain.len() || shared >= target_chain.len() {
            continue;
        }

        let (Some(&parent), Some(&from), Some(&to)) = (
            source_chain.get(shared - 1),
            source_chain.get(shared),
            target_chain.get(shared),
        ) else {
            continue;
        };

        // A module declared inside a function body is a local implementation
        // detail rather than a node in the crate's architecture.
        if tcx.def_kind(parent) != DefKind::Mod {
            continue;
        }

        graphs.entry(parent).or_default().push(SiblingEdge {
            from,
            to,
            span: reference.span,
        });
    }

    graphs
}

fn report_cycles(cx: &LateContext<'_>, parent: DefId, edges: &[SiblingEdge]) {
    let mut nodes: Vec<DefId> = edges
        .iter()
        .flat_map(|edge| [edge.from, edge.to])
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    nodes.sort_by_cached_key(|node| module_name(cx.tcx, *node));

    let mut adjacency: HashMap<DefId, Vec<DefId>> = HashMap::new();
    let mut seen: HashSet<(DefId, DefId)> = HashSet::new();
    let mut witnesses: HashMap<(DefId, DefId), Span> = HashMap::new();
    for node in &nodes {
        adjacency.insert(*node, Vec::new());
    }
    for edge in edges {
        if seen.insert((edge.from, edge.to))
            && let Some(neighbors) = adjacency.get_mut(&edge.from)
        {
            neighbors.push(edge.to);
        }
        // The first reference to witness an edge is the one the diagnostic
        // points at, so a module with many crossings still reports one span per
        // direction.
        witnesses.entry((edge.from, edge.to)).or_insert(edge.span);
    }
    for neighbors in adjacency.values_mut() {
        neighbors.sort_by_cached_key(|node| module_name(cx.tcx, *node));
    }

    for cycle in detect_cycles(&nodes, &adjacency) {
        emit_cycle(cx, parent, &cycle, &witnesses);
    }
}

/// Finds every cycle reachable in `adjacency` by depth-first search, colouring
/// nodes grey while they sit on the current path and black once finished: an
/// edge back into a grey node closes a cycle.
///
/// `nodes` fixes both the traversal order and the rotation each cycle is
/// normalized to, so a cycle is reported identically no matter which of its
/// members the search entered from.
fn detect_cycles<N>(nodes: &[N], adjacency: &HashMap<N, Vec<N>>) -> Vec<Vec<N>>
where
    N: Copy + Eq + Hash,
{
    let order: HashMap<N, usize> = nodes
        .iter()
        .enumerate()
        .map(|(index, node)| (*node, index))
        .collect();

    let mut state: HashMap<N, Color> = HashMap::new();
    let mut path: Vec<N> = Vec::new();
    let mut cycles: Vec<Vec<N>> = Vec::new();

    for node in nodes {
        if !state.contains_key(node) {
            visit(*node, adjacency, &mut state, &mut path, &mut cycles);
        }
    }

    let mut normalized: Vec<Vec<N>> = cycles
        .iter()
        .map(|cycle| normalize_cycle(cycle, &order))
        .collect();
    normalized.sort_by_cached_key(|cycle| cycle_key(cycle, &order));
    normalized.dedup_by_key(|cycle| cycle_key(cycle, &order));
    normalized
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Color {
    OnPath,
    Finished,
}

fn visit<N>(
    node: N,
    adjacency: &HashMap<N, Vec<N>>,
    state: &mut HashMap<N, Color>,
    path: &mut Vec<N>,
    cycles: &mut Vec<Vec<N>>,
) where
    N: Copy + Eq + Hash,
{
    state.insert(node, Color::OnPath);
    path.push(node);

    for next in adjacency.get(&node).into_iter().flatten() {
        match state.get(next) {
            None => visit(*next, adjacency, state, path, cycles),
            Some(Color::OnPath) => {
                if let Some(start) = path.iter().position(|entry| entry == next) {
                    cycles.push(path[start..].to_vec());
                }
            }
            Some(Color::Finished) => {}
        }
    }

    path.pop();
    state.insert(node, Color::Finished);
}

fn normalize_cycle<N>(cycle: &[N], order: &HashMap<N, usize>) -> Vec<N>
where
    N: Copy + Eq + Hash,
{
    let start = cycle
        .iter()
        .enumerate()
        .min_by_key(|(_, node)| order.get(node).copied().unwrap_or(usize::MAX))
        .map_or(0, |(index, _)| index);

    cycle
        .iter()
        .cycle()
        .skip(start)
        .take(cycle.len())
        .copied()
        .collect()
}

fn cycle_key<N>(cycle: &[N], order: &HashMap<N, usize>) -> Vec<usize>
where
    N: Copy + Eq + Hash,
{
    cycle
        .iter()
        .map(|node| order.get(node).copied().unwrap_or(usize::MAX))
        .collect()
}

fn emit_cycle(
    cx: &LateContext<'_>,
    parent: DefId,
    cycle: &[DefId],
    witnesses: &HashMap<(DefId, DefId), Span>,
) {
    let names: Vec<String> = cycle.iter().map(|node| module_name(cx.tcx, *node)).collect();
    let Some(first) = names.first() else {
        return;
    };

    let arrows = names
        .iter()
        .chain(std::iter::once(first))
        .map(|name| format!("`{name}`"))
        .collect::<Vec<_>>()
        .join(" -> ");

    let spans: Vec<(String, String, Span)> = cycle
        .iter()
        .zip(cycle.iter().cycle().skip(1))
        .zip(names.iter().zip(names.iter().cycle().skip(1)))
        .filter_map(|((from, to), (from_name, to_name))| {
            witnesses
                .get(&(*from, *to))
                .map(|span| (from_name.clone(), to_name.clone(), *span))
        })
        .collect();

    let Some(&(_, _, primary)) = spans.first() else {
        return;
    };

    // The parent module owns the cycle, so the diagnostic is levelled there:
    // an `allow`/`expect` on the module that declares the siblings covers it,
    // wherever the individual references happen to sit.
    let attribution = parent
        .as_local()
        .map_or(rustc_hir::CRATE_HIR_ID, |local| {
            cx.tcx.local_def_id_to_hir_id(local)
        });

    let parent_path = module_path(cx.tcx, parent);

    span_lint_hir_and_then(
        cx,
        ACYCLIC_MODULES,
        attribution,
        primary,
        format!("cyclic dependency between sibling modules under `{parent_path}`: {arrows}"),
        |diag| {
            for (from_name, to_name, span) in &spans {
                diag.span_label(*span, format!("`{from_name}` -> `{to_name}`"));
            }

            if let [left, right] = names.as_slice() {
                diag.help(format!(
                    "move the items `{left}` and `{right}` share into a module they can both \
                     depend on, or turn one of the two references around so the dependency \
                     flows in one direction"
                ));
            } else {
                let listed = names
                    .iter()
                    .map(|name| format!("`{name}`"))
                    .collect::<Vec<_>>()
                    .join(", ");
                diag.help(format!(
                    "extract the items {listed} share into a module they can all depend on, so \
                     the dependency flows in one direction"
                ));
            }

            diag.note(format!(
                "if the coupling is deliberate, opt out on `{parent_path}` with \
                 `#[cfg_attr(dylint_lib = \"trogon_lints\", expect(acyclic_modules, reason = \"...\"))]`"
            ));
        },
    );
}

/// The module a definition belongs to: the definition itself when it is a
/// module, otherwise its nearest module ancestor.
fn enclosing_module(tcx: TyCtxt<'_>, def_id: DefId) -> DefId {
    let mut current = def_id;
    while tcx.def_kind(current) != DefKind::Mod {
        let Some(parent) = tcx.opt_parent(current) else {
            return current;
        };
        current = parent;
    }
    current
}

/// The module's ancestors from the crate root down to and including itself.
fn module_chain(tcx: TyCtxt<'_>, module: DefId) -> Vec<DefId> {
    let mut chain = vec![module];
    let mut current = module;
    while let Some(parent) = tcx.opt_parent(current) {
        chain.push(parent);
        current = parent;
    }
    chain.reverse();
    chain
}

fn is_crate_root(def_id: DefId) -> bool {
    def_id.index == CRATE_DEF_INDEX
}

fn module_name(tcx: TyCtxt<'_>, module: DefId) -> String {
    if is_crate_root(module) {
        return "crate".to_owned();
    }
    tcx.opt_item_name(module)
        .map_or_else(|| "_".to_owned(), |name| name.to_string())
}

fn module_path(tcx: TyCtxt<'_>, module: DefId) -> String {
    if is_crate_root(module) {
        return "crate".to_owned();
    }
    module_chain(tcx, module)
        .iter()
        .filter(|ancestor| !is_crate_root(**ancestor))
        .map(|ancestor| module_name(tcx, *ancestor))
        .collect::<Vec<_>>()
        .join("::")
}

/// Whether the module, or any module above it, is one of the test and benchmark
/// module families. A fixture reaches across the tree by design, and the
/// coupling it creates says nothing about how production code is layered.
fn is_test_module_subtree(tcx: TyCtxt<'_>, module: DefId) -> bool {
    module_chain(tcx, module).iter().any(|ancestor| {
        !is_crate_root(*ancestor)
            && tcx
                .opt_item_name(*ancestor)
                .is_some_and(|name| is_test_module_name(name.as_str()))
    })
}

/// Whether the reference sits in test code, where reaching across module
/// boundaries is expected. `is_test_context` cannot answer this, because a
/// `use` item or a field type has no enclosing body; the module-family part of
/// the question is answered from the module tree instead, by
/// `is_test_module_subtree`.
fn is_test_context(cx: &LateContext<'_>, hir_id: HirId, span: Span) -> bool {
    is_in_test(cx.tcx, hir_id) || is_test_or_bench_source(cx, span)
}

// The graph work is the part of this lint that can be wrong without the
// compiler noticing, and it is expressible without a `TyCtxt`, so it is tested
// against plain names rather than through the compiletest fixture.
#[cfg(test)]
mod tests {
    use super::detect_cycles;
    use std::collections::HashMap;

    fn cycles(nodes: &[&'static str], edges: &[(&'static str, &'static str)]) -> Vec<Vec<String>> {
        let mut adjacency: HashMap<&'static str, Vec<&'static str>> = HashMap::new();
        for node in nodes {
            adjacency.insert(node, Vec::new());
        }
        for (from, to) in edges {
            if let Some(neighbors) = adjacency.get_mut(from)
                && !neighbors.contains(to)
            {
                neighbors.push(to);
            }
        }
        for neighbors in adjacency.values_mut() {
            neighbors.sort_unstable();
        }

        detect_cycles(nodes, &adjacency)
            .iter()
            .map(|cycle| cycle.iter().map(|node| (*node).to_owned()).collect())
            .collect()
    }

    #[test]
    fn reports_a_mutual_dependency() {
        assert_eq!(
            cycles(
                &["payments", "server"],
                &[("payments", "server"), ("server", "payments")]
            ),
            vec![vec!["payments", "server"]]
        );
    }

    #[test]
    fn reports_a_cycle_longer_than_a_pair() {
        assert_eq!(
            cycles(&["a", "b", "c"], &[("a", "b"), ("b", "c"), ("c", "a")]),
            vec![vec!["a", "b", "c"]]
        );
    }

    #[test]
    fn accepts_a_one_directional_dependency() {
        assert!(
            cycles(
                &["consumer", "utils"],
                &[("consumer", "utils"), ("consumer", "utils")]
            )
            .is_empty()
        );
    }

    #[test]
    fn accepts_a_diamond() {
        assert!(
            cycles(
                &["base", "left", "right", "top"],
                &[
                    ("top", "left"),
                    ("top", "right"),
                    ("left", "base"),
                    ("right", "base"),
                ]
            )
            .is_empty()
        );
    }

    #[test]
    fn reports_each_disjoint_cycle_once() {
        assert_eq!(
            cycles(
                &["a", "b", "c", "d"],
                &[("a", "b"), ("b", "a"), ("c", "d"), ("d", "c")]
            ),
            vec![vec!["a", "b"], vec!["c", "d"]]
        );
    }

    // The same cycle is reachable from either of its members, and which edge
    // happened to be recorded first must not change how it is spelled, or the
    // same crate would report a different cycle from build to build.
    #[test]
    fn spells_a_cycle_the_same_way_whichever_edge_came_first() {
        let forward = cycles(&["a", "b"], &[("a", "b"), ("b", "a")]);
        let reversed = cycles(&["a", "b"], &[("b", "a"), ("a", "b")]);
        assert_eq!(forward, vec![vec!["a", "b"]]);
        assert_eq!(reversed, forward);
    }
}
