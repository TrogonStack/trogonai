import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { describe, expect, it } from 'vitest';

import { applyFilters, emptyFilterState } from './apply-filters';
import { FilterBar } from './filter-bar';
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
    icon: <span />,
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
    icon: <span />,
    accessor: (row) => row.vault,
    values: [
      { value: 'production', label: 'Production' },
      { value: 'staging', label: 'Staging' },
    ],
  },
];

const matchesText = (row: Row, needle: string) => row.name.toLowerCase().includes(needle);

function Harness({ initial = emptyFilterState }: { initial?: FilterState }) {
  const [state, setState] = useState(initial);
  const visible = applyFilters(rows, state, facets, matchesText);
  return (
    <>
      <FilterBar
        label="Filter keys"
        rows={rows}
        facets={facets}
        state={state}
        onChange={setState}
        matchesText={matchesText}
      />
      <ul aria-label="Visible rows">
        {visible.map((row) => (
          <li key={row.name}>{row.name}</li>
        ))}
      </ul>
    </>
  );
}

const names = () =>
  within(screen.getByRole('list', { name: 'Visible rows' }))
    .queryAllByRole('listitem')
    .map((item) => item.textContent);

describe('FilterBar', () => {
  it('lists the facets when the empty input is focused', async () => {
    const user = userEvent.setup();
    render(<Harness />);

    await user.click(screen.getByRole('combobox', { name: 'Filter keys' }));

    const options = await screen.findAllByRole('option');
    expect(options.map((option) => option.textContent)).toEqual(['Status', 'Vault']);
  });

  it('drives the whole flow from the keyboard', async () => {
    const user = userEvent.setup();
    render(<Harness />);

    await user.click(screen.getByRole('combobox', { name: 'Filter keys' }));
    await user.keyboard('{Enter}');
    const panel = await screen.findByRole('combobox', { name: 'Filter Status' });
    expect(panel).toHaveFocus();

    await user.keyboard('{Enter}{ArrowDown}{Enter}');
    expect(names()).toEqual(['Alpha key', 'Beta key', 'Gamma key']);
    expect(screen.getByRole('button', { name: 'Remove Status filter' })).toBeInTheDocument();
    expect(screen.getByText('is any of')).toBeInTheDocument();

    await user.keyboard('{Backspace}');
    expect(screen.getByRole('combobox', { name: 'Filter keys' })).toHaveFocus();
    expect(names()).toEqual(['Alpha key', 'Beta key', 'Gamma key']);

    await user.keyboard('{Escape}');
    await waitFor(() => expect(screen.queryByRole('listbox')).not.toBeInTheDocument());
    expect(names()).toEqual(['Alpha key', 'Beta key', 'Gamma key']);
  });

  it('cancels the selections made in a value list with Escape and returns to the facets', async () => {
    const user = userEvent.setup();
    render(<Harness initial={{ text: '', selected: { status: ['active'] } }} />);

    await user.click(screen.getByRole('button', { name: 'Edit Status filter' }));
    await user.click(await screen.findByRole('option', { name: 'Revoked' }));
    await user.click(screen.getByRole('option', { name: 'Active' }));
    expect(names()).toEqual(['Delta key']);

    await user.keyboard('{Escape}');

    expect(names()).toEqual(['Alpha key', 'Gamma key']);
    expect(screen.getByRole('combobox', { name: 'Filter keys' })).toHaveFocus();
    expect(screen.getByRole('listbox', { name: 'Filter suggestions' })).toBeInTheDocument();
    expect(screen.getAllByRole('option').map((option) => option.textContent)).toEqual(['Status', 'Vault']);

    await user.keyboard('{Escape}');
    await waitFor(() => expect(screen.queryByRole('listbox')).not.toBeInTheDocument());
    expect(names()).toEqual(['Alpha key', 'Gamma key']);
  });

  it('removes a new facet again when its value list is cancelled', async () => {
    const user = userEvent.setup();
    render(<Harness />);

    await user.click(screen.getByRole('combobox', { name: 'Filter keys' }));
    await user.click(await screen.findByRole('option', { name: 'Vault' }));
    await user.click(await screen.findByRole('option', { name: 'Staging' }));
    expect(screen.getByRole('button', { name: 'Remove Vault filter' })).toBeInTheDocument();

    await user.keyboard('{Escape}');

    expect(screen.queryByRole('button', { name: 'Remove Vault filter' })).not.toBeInTheDocument();
    expect(names()).toHaveLength(4);
  });

  it('suggests matching values across facets while typing', async () => {
    const user = userEvent.setup();
    render(<Harness />);

    await user.type(screen.getByRole('combobox', { name: 'Filter keys' }), 'prod');

    const options = await screen.findAllByRole('option');
    expect(options.map((option) => option.getAttribute('aria-label') ?? option.textContent)).toEqual([
      'Vault: Production',
      expect.stringContaining('prod'),
    ]);

    await user.click(screen.getByRole('option', { name: 'Vault: Production' }));
    expect(names()).toEqual(['Alpha key', 'Beta key']);
  });

  it('suggests a status for a partial value', async () => {
    const user = userEvent.setup();
    render(<Harness />);

    await user.type(screen.getByRole('combobox', { name: 'Filter keys' }), 'rev');

    expect(await screen.findByRole('option', { name: 'Status: Revoked' })).toBeInTheDocument();
  });

  it('applies a matching value from the first suggestion', async () => {
    const user = userEvent.setup();
    render(<Harness />);

    await user.type(screen.getByRole('combobox', { name: 'Filter keys' }), 'revoked{Enter}');

    expect(screen.getByRole('button', { name: 'Remove Status filter' })).toBeInTheDocument();
  });

  it('applies free text when no value matches', async () => {
    const user = userEvent.setup();
    render(<Harness />);

    await user.type(screen.getByRole('combobox', { name: 'Filter keys' }), 'gamma{Enter}');

    expect(names()).toEqual(['Gamma key']);
    expect(screen.getByRole('button', { name: 'Remove Search filter' })).toBeInTheDocument();
  });

  it('removes the last pill with Backspace on an empty input', async () => {
    const user = userEvent.setup();
    render(<Harness initial={{ text: '', selected: { status: ['active'], vault: ['staging'] } }} />);

    expect(names()).toEqual(['Gamma key']);
    await user.click(screen.getByRole('combobox', { name: 'Filter keys' }));
    await user.keyboard('{Backspace}');

    expect(screen.queryByRole('button', { name: 'Remove Vault filter' })).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Remove Status filter' })).toBeInTheDocument();
    expect(names()).toEqual(['Alpha key', 'Gamma key']);
  });

  it('reopens the value list from the pill and keeps it open while toggling', async () => {
    const user = userEvent.setup();
    render(<Harness initial={{ text: '', selected: { status: ['active'] } }} />);

    await user.click(screen.getByRole('button', { name: 'Edit Status filter' }));
    await user.click(await screen.findByRole('option', { name: 'Revoked' }));

    expect(names()).toEqual(['Alpha key', 'Gamma key', 'Delta key']);
    expect(screen.getByRole('listbox', { name: 'Status values' })).toBeInTheDocument();
    expect(screen.getByRole('option', { name: 'Revoked' })).toHaveAttribute('aria-selected', 'true');
  });

  it('shows counts that reflect the other active filters', async () => {
    const user = userEvent.setup();
    render(<Harness initial={{ text: '', selected: { vault: ['staging'] } }} />);

    await user.click(screen.getByRole('combobox', { name: 'Filter keys' }));
    await user.click(await screen.findByRole('option', { name: 'Status' }));

    const active = await screen.findByRole('option', { name: 'Active' });
    expect(within(active).getByText('1')).toBeInTheDocument();
    expect(within(screen.getByRole('option', { name: 'Disabled' })).getByText('0')).toBeInTheDocument();
    expect(within(screen.getByRole('option', { name: 'Revoked' })).getByText('1')).toBeInTheDocument();
  });

  it('clears every filter from the trailing button', async () => {
    const user = userEvent.setup();
    render(<Harness initial={{ text: 'key', selected: { status: ['active'] } }} />);

    await user.click(screen.getByRole('button', { name: 'Clear all filters' }));

    expect(names()).toHaveLength(4);
    expect(screen.queryByRole('button', { name: 'Clear all filters' })).not.toBeInTheDocument();
  });

  it('focuses the bar when F is pressed outside a field', async () => {
    const user = userEvent.setup();
    render(<Harness />);

    await user.keyboard('f');

    expect(screen.getByRole('combobox', { name: 'Filter keys' })).toHaveFocus();
  });
});
