import type { ReactNode } from 'react';

import type { FilterFacet, FilterValue } from './types';

interface FacetSpec<Row> {
  key: string;
  label: string;
  icon: ReactNode;
  rows: readonly Row[];
  accessor: FilterFacet<Row>['accessor'];
  describe: (value: string) => Pick<FilterValue, 'label' | 'icon'>;
}

export function facetFromRows<Row>({ key, label, icon, rows, accessor, describe }: FacetSpec<Row>): FilterFacet<Row> {
  const present = [...new Set(rows.flatMap((row) => accessor(row)))];
  const values = present
    .map((value) => ({ value, ...describe(value) }))
    .sort((left, right) => left.label.localeCompare(right.label));
  return { key, label, icon, accessor, values };
}
