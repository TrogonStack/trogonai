import { describe, expect, it } from 'vitest';

import {
  applyFilters,
  emptyFilterState,
  facetCounts,
  isFiltering,
  removeFacet,
  setText,
  toggleValue,
} from './apply-filters';
import { suggestionItems, valueItems } from './suggestions';
import type { FilterFacet, FilterState } from './types';

interface Row {
  readonly name: string;
  readonly status: string;
  readonly vault: string;
}

const rows: readonly Row[] = [
  { name: 'Alpha key', status: 'active', vault: 'production' },
  { name: 'Beta key', status: 'disabled', vault: 'production' },
  { name: 'Gamma key', status: 'active', vault: 'staging' },
  { name: 'Delta key', status: 'revoked', vault: 'staging' },
];

const facets: readonly FilterFacet<Row>[] = [
  {
    key: 'status',
    label: 'Status',
    icon: null,
    accessor: (row) => row.status,
    values: [
      { value: 'active', label: 'Active' },
      { value: 'disabled', label: 'Disabled' },
      { value: 'revoked', label: 'Revoked' },
    ],
  },
  {
    key: 'vault',
    label: 'Vault',
    icon: null,
    accessor: (row) => row.vault,
    values: [
      { value: 'production', label: 'Production' },
      { value: 'staging', label: 'Staging' },
    ],
  },
];

const matcher = (row: Row, needle: string) => row.name.toLowerCase().includes(needle);

const select = (selected: FilterState['selected'], text = ''): FilterState => ({ text, selected });

describe('applyFilters', () => {
  it('returns every row for the empty state', () => {
    expect(applyFilters(rows, emptyFilterState, facets, matcher)).toEqual(rows);
  });

  it('ORs the values within a facet', () => {
    const result = applyFilters(rows, select({ status: ['active', 'disabled'] }), facets, matcher);
    expect(result.map((row) => row.name)).toEqual(['Alpha key', 'Beta key', 'Gamma key']);
  });

  it('ANDs the facets together', () => {
    const result = applyFilters(rows, select({ status: ['active'], vault: ['staging'] }), facets, matcher);
    expect(result.map((row) => row.name)).toEqual(['Gamma key']);
  });

  it('ANDs the free text with the facets', () => {
    const result = applyFilters(rows, select({ status: ['active'] }, ' ALPHA '), facets, matcher);
    expect(result.map((row) => row.name)).toEqual(['Alpha key']);
  });

  it('ignores free text without a matcher', () => {
    expect(applyFilters(rows, select({}, 'zzz'), facets)).toEqual(rows);
  });

  it('treats a facet with no selected values as unfiltered', () => {
    expect(applyFilters(rows, select({ status: [] }), facets, matcher)).toEqual(rows);
  });
});

describe('multi-valued facets', () => {
  interface Tagged {
    readonly tags: readonly string[];
  }
  const tagged: readonly Tagged[] = [{ tags: ['a', 'b'] }, { tags: ['b'] }, { tags: [] }];
  const tagFacets: readonly FilterFacet<Tagged>[] = [
    {
      key: 'tag',
      label: 'Tag',
      icon: null,
      accessor: (row) => row.tags,
      values: [
        { value: 'a', label: 'A' },
        { value: 'b', label: 'B' },
      ],
    },
  ];

  it('matches a row when any of its values is selected', () => {
    expect(applyFilters(tagged, select({ tag: ['a'] }), tagFacets)).toEqual([tagged[0]]);
    expect(applyFilters(tagged, select({ tag: ['b'] }), tagFacets)).toEqual([tagged[0], tagged[1]]);
  });

  it('counts a row once per value it carries', () => {
    const counts = facetCounts(tagged, emptyFilterState, tagFacets);
    expect(Object.fromEntries(counts.get('tag')!)).toEqual({ a: 1, b: 2 });
  });
});

describe('facetCounts', () => {
  it('counts every value against the other active filters', () => {
    const counts = facetCounts(rows, select({ vault: ['staging'] }), facets, matcher);
    expect(Object.fromEntries(counts.get('status')!)).toEqual({ active: 1, disabled: 0, revoked: 1 });
  });

  it('does not narrow a facet by its own selection', () => {
    const counts = facetCounts(rows, select({ status: ['active'] }), facets, matcher);
    expect(Object.fromEntries(counts.get('status')!)).toEqual({ active: 2, disabled: 1, revoked: 1 });
    expect(Object.fromEntries(counts.get('vault')!)).toEqual({ production: 1, staging: 1 });
  });

  it('applies the free text to every facet', () => {
    const counts = facetCounts(rows, select({}, 'beta'), facets, matcher);
    expect(Object.fromEntries(counts.get('vault')!)).toEqual({ production: 1, staging: 0 });
  });
});

describe('state helpers', () => {
  it('toggles a value on and off and drops an emptied facet', () => {
    const on = toggleValue(emptyFilterState, 'status', 'active');
    expect(on.selected).toEqual({ status: ['active'] });
    const more = toggleValue(on, 'status', 'revoked');
    expect(more.selected).toEqual({ status: ['active', 'revoked'] });
    const off = toggleValue(toggleValue(more, 'status', 'revoked'), 'status', 'active');
    expect(off.selected).toEqual({});
  });

  it('removes a facet and trims the text', () => {
    const state = select({ status: ['active'], vault: ['staging'] });
    expect(removeFacet(state, 'status').selected).toEqual({ vault: ['staging'] });
    expect(setText(state, '  hi ').text).toBe('hi');
  });

  it('reports whether anything is applied', () => {
    expect(isFiltering(emptyFilterState)).toBe(false);
    expect(isFiltering(select({}, 'x'))).toBe(true);
    expect(isFiltering(select({ status: ['active'] }))).toBe(true);
  });
});

describe('suggestions', () => {
  const counts = facetCounts(rows, emptyFilterState, facets, matcher);

  it('lists the facets for an empty query', () => {
    const items = suggestionItems('', facets, counts, emptyFilterState);
    expect(items.map((item) => item.kind)).toEqual(['facet', 'facet']);
  });

  it('offers matching values, then matching facets, then the search row', () => {
    const items = suggestionItems('prod', facets, counts, emptyFilterState);
    expect(items.map((item) => item.id)).toEqual(['value:vault:production', 'search']);
    expect(items[0]).toMatchObject({ kind: 'value', count: 2, checked: false });
  });

  it('matches a facet by name', () => {
    const items = suggestionItems('stat', facets, counts, emptyFilterState);
    expect(items.map((item) => item.id)).toEqual(['facet:status', 'search']);
  });

  it('marks values that are already selected', () => {
    const state = select({ status: ['revoked'] });
    const items = suggestionItems('rev', facets, counts, state);
    expect(items[0]).toMatchObject({ id: 'value:status:revoked', checked: true });
  });

  it('filters the values of one facet', () => {
    const items = valueItems(facets[0]!, 'dis', counts, emptyFilterState);
    expect(items.map((item) => item.id)).toEqual(['value:status:disabled']);
  });
});
