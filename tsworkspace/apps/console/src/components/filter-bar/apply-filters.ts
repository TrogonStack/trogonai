import type { FacetCounts, FilterFacet, FilterState, TextMatcher } from './types';

export const emptyFilterState: FilterState = { text: '', selected: {} };

export function isFiltering(state: FilterState): boolean {
  return state.text !== '' || Object.keys(state.selected).length > 0;
}

export function toggleValue(state: FilterState, facetKey: string, value: string): FilterState {
  const current = state.selected[facetKey] ?? [];
  const next = current.includes(value) ? current.filter((item) => item !== value) : [...current, value];
  const { [facetKey]: _removed, ...rest } = state.selected;
  return { ...state, selected: next.length === 0 ? rest : { ...rest, [facetKey]: next } };
}

export function removeFacet(state: FilterState, facetKey: string): FilterState {
  const { [facetKey]: _removed, ...rest } = state.selected;
  return { ...state, selected: rest };
}

export function setText(state: FilterState, text: string): FilterState {
  return { ...state, text: text.trim() };
}

function valuesOf<Row>(facet: FilterFacet<Row>, row: Row): readonly string[] {
  const value = facet.accessor(row);
  return typeof value === 'string' ? [value] : value;
}

function matchesFacets<Row>(row: Row, state: FilterState, facets: readonly FilterFacet<Row>[], skip?: string): boolean {
  return facets.every((facet) => {
    if (facet.key === skip) return true;
    const selected = state.selected[facet.key];
    return (
      selected === undefined || selected.length === 0 || valuesOf(facet, row).some((value) => selected.includes(value))
    );
  });
}

function matchesText<Row>(row: Row, state: FilterState, matcher: TextMatcher<Row> | undefined): boolean {
  const needle = state.text.trim().toLowerCase();
  return needle === '' || matcher === undefined || matcher(row, needle);
}

export function applyFilters<Row>(
  rows: readonly Row[],
  state: FilterState,
  facets: readonly FilterFacet<Row>[],
  matcher?: TextMatcher<Row>,
): Row[] {
  return rows.filter((row) => matchesFacets(row, state, facets) && matchesText(row, state, matcher));
}

export function facetCounts<Row>(
  rows: readonly Row[],
  state: FilterState,
  facets: readonly FilterFacet<Row>[],
  matcher?: TextMatcher<Row>,
): FacetCounts {
  const result = new Map<string, ReadonlyMap<string, number>>();
  for (const facet of facets) {
    const counts = new Map<string, number>(facet.values.map((value) => [value.value, 0]));
    for (const row of rows) {
      if (!matchesFacets(row, state, facets, facet.key) || !matchesText(row, state, matcher)) continue;
      for (const value of valuesOf(facet, row)) counts.set(value, (counts.get(value) ?? 0) + 1);
    }
    result.set(facet.key, counts);
  }
  return result;
}
