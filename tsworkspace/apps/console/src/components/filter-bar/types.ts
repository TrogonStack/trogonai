import type { ReactNode } from 'react';

export interface FilterValue {
  readonly value: string;
  readonly label: string;
  readonly icon?: ReactNode;
}

export interface FilterFacet<Row> {
  readonly key: string;
  readonly label: string;
  readonly icon: ReactNode;
  readonly values: readonly FilterValue[];
  readonly accessor: (row: Row) => string | readonly string[];
}

export interface FilterState {
  readonly text: string;
  readonly selected: Readonly<Record<string, readonly string[]>>;
}

export type TextMatcher<Row> = (row: Row, needle: string) => boolean;

export type FacetCounts = ReadonlyMap<string, ReadonlyMap<string, number>>;
