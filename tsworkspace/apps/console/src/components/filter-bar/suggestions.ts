import type { FacetCounts, FilterFacet, FilterState, FilterValue } from './types';

export type FilterItem<Row> =
  | { readonly kind: 'search'; readonly id: string; readonly text: string }
  | { readonly kind: 'facet'; readonly id: string; readonly facet: FilterFacet<Row> }
  | {
      readonly kind: 'value';
      readonly id: string;
      readonly facet: FilterFacet<Row>;
      readonly option: FilterValue;
      readonly count: number;
      readonly checked: boolean;
    };

function valueItem<Row>(
  facet: FilterFacet<Row>,
  option: FilterValue,
  counts: FacetCounts,
  state: FilterState,
): FilterItem<Row> {
  return {
    kind: 'value',
    id: `value:${facet.key}:${option.value}`,
    facet,
    option,
    count: counts.get(facet.key)?.get(option.value) ?? 0,
    checked: state.selected[facet.key]?.includes(option.value) ?? false,
  };
}

export function facetItems<Row>(facets: readonly FilterFacet<Row>[]): FilterItem<Row>[] {
  return facets.map((facet) => ({ kind: 'facet', id: `facet:${facet.key}`, facet }));
}

export function suggestionItems<Row>(
  query: string,
  facets: readonly FilterFacet<Row>[],
  counts: FacetCounts,
  state: FilterState,
): FilterItem<Row>[] {
  const needle = query.trim().toLowerCase();
  if (needle === '') return facetItems(facets);
  const matchingFacets = facets.filter((facet) => facet.label.toLowerCase().includes(needle));
  const matchingValues = facets.flatMap((facet) =>
    facet.values
      .filter((option) => option.label.toLowerCase().includes(needle))
      .map((option) => valueItem(facet, option, counts, state)),
  );
  return [...matchingValues, ...facetItems(matchingFacets), { kind: 'search', id: 'search', text: query.trim() }];
}

export function valueItems<Row>(
  facet: FilterFacet<Row>,
  query: string,
  counts: FacetCounts,
  state: FilterState,
): FilterItem<Row>[] {
  const needle = query.trim().toLowerCase();
  return facet.values
    .filter((option) => needle === '' || option.label.toLowerCase().includes(needle))
    .map((option) => valueItem(facet, option, counts, state));
}
