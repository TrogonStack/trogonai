import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { describe, expect, it, vi } from 'vitest';

import { ExceptionDates } from './exception-dates';

const first = '2027-01-01T09:00:00Z';
function Harness({ initial = '', opposing = '' }: { initial?: string; opposing?: string }) {
  const [value, onChange] = useState(initial);
  return (
    <>
      <ExceptionDates id="rdate" label="Included dates" value={value} onChange={onChange} opposingValue={opposing} />
      <output data-testid="canonical">{value}</output>
    </>
  );
}
async function addPrecise(user: ReturnType<typeof userEvent.setup>, value: string) {
  if (screen.queryByRole('button', { name: 'Use precise ISO for included dates' }))
    await user.click(screen.getByRole('button', { name: 'Use precise ISO for included dates' }));
  await user.clear(screen.getByLabelText('New included date (ISO)'));
  await user.type(screen.getByLabelText('New included date (ISO)'), value);
  await user.click(screen.getByRole('button', { name: 'Add included date' }));
}

describe('exception date list manager', () => {
  it('starts empty and adds a UTC picker date as a compact row', async () => {
    const user = userEvent.setup();
    render(<Harness />);
    expect(screen.getByText('No included dates yet')).toBeInTheDocument();
    expect(screen.queryByRole('textbox', { name: 'Included dates' })).not.toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'New included date (UTC)' }));
    const today = new Date();
    const day = screen.getAllByRole('button').find((button) => button.getAttribute('aria-label')?.startsWith('Today,'));
    expect(day).toBeDefined();
    await user.click(day!);
    await user.click(screen.getByRole('button', { name: 'Apply date and time' }));
    await user.click(screen.getByRole('button', { name: 'Add included date' }));
    expect(screen.getByText('00:00:00 UTC')).toBeInTheDocument();
    expect(screen.getByTestId('canonical').textContent).toBe(`${today.toISOString().slice(0, 10)}T00:00:00Z`);
  });
  it('blocks equivalent-offset duplicates while allowing distinct nanoseconds', async () => {
    const user = userEvent.setup();
    render(<Harness initial="2027-01-01T09:00:00.000000001Z" />);
    await addPrecise(user, '2027-01-01T10:00:00.000000001+01:00');
    expect(screen.getByText(/already in the list, even/)).toBeInTheDocument();
    expect(within(screen.getByRole('list', { name: 'Included dates entries' })).getAllByRole('listitem')).toHaveLength(
      1,
    );
    await addPrecise(user, '2027-01-01T09:00:00.000000002Z');
    expect(within(screen.getByRole('list', { name: 'Included dates entries' })).getAllByRole('listitem')).toHaveLength(
      2,
    );
  });
  it('shows exclusion conflicts and existing invalid and duplicate entries without deleting them', () => {
    render(<Harness initial={`${first}\n2027-01-01T10:00:00+01:00\nbad-date`} opposing="2027-01-01T10:00:00+01:00" />);
    expect(screen.getAllByText(/Also excluded/)).toHaveLength(2);
    expect(screen.getAllByText('Duplicate instant in this list.')).toHaveLength(2);
    expect(screen.getByText('Invalid date')).toBeInTheDocument();
    expect(screen.getByTestId('canonical').textContent).toBe(`${first}\n2027-01-01T10:00:00+01:00\nbad-date`);
  });
  it('edits in a UTC picker, cancels unchanged, and preserves unrelated precise strings', async () => {
    const user = userEvent.setup();
    const preserved = '2027-01-02T10:00:00.123456789+01:00';
    render(<Harness initial={`${first}\n${preserved}`} />);
    await user.click(screen.getByRole('button', { name: `Edit ${first}` }));
    expect(screen.getByRole('button', { name: 'Edit included date' })).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Edit included date' }));
    await user.click(screen.getByRole('button', { name: 'Sunday, January 3rd, 2027' }));
    await user.click(screen.getByRole('button', { name: 'Apply date and time' }));
    await user.click(screen.getByRole('button', { name: 'Cancel edit' }));
    expect(screen.getByTestId('canonical').textContent).toBe(`${first}\n${preserved}`);
    await user.click(screen.getByRole('button', { name: `Edit ${first}` }));
    await user.click(screen.getByRole('button', { name: 'Edit included date' }));
    await user.click(screen.getByRole('button', { name: 'Sunday, January 3rd, 2027' }));
    await user.click(screen.getByRole('button', { name: 'Apply date and time' }));
    await user.click(screen.getByRole('button', { name: 'Save date' }));
    expect(screen.getByTestId('canonical').textContent).toBe(`2027-01-03T09:00:00Z\n${preserved}`);
    const items = within(screen.getByRole('list', { name: 'Included dates entries' })).getAllByRole('listitem');
    expect(within(items[0]!).getByText('Jan 2, 2027')).toBeInTheDocument();
  });
  it('repairs invalid entries and targets the correct row when another row is removed during edit', async () => {
    const user = userEvent.setup();
    render(<Harness initial={`invalid\n${first}`} />);
    await user.click(screen.getByRole('button', { name: 'Edit invalid' }));
    await user.clear(screen.getByLabelText('Edit included date'));
    await user.type(screen.getByLabelText('Edit included date'), '2027-01-02T09:00:00.000000001Z');
    await user.click(screen.getByRole('button', { name: `Remove ${first}` }));
    await user.click(screen.getByRole('button', { name: 'Save date' }));
    expect(screen.getByTestId('canonical').textContent).toBe('2027-01-02T09:00:00.000000001Z');
    await user.click(screen.getByRole('button', { name: 'Edit 2027-01-02T09:00:00.000000001Z' }));
    await user.keyboard('{Escape}');
    expect(screen.queryByLabelText('Edit included date')).not.toBeInTheDocument();
  });
  it('imports only after the entire batch validates and explicitly reports duplicates', async () => {
    const user = userEvent.setup();
    render(<Harness initial={first} />);
    await user.click(screen.getByText('Import dates', { selector: 'summary' }));
    const input = screen.getByLabelText('Paste included dates');
    await user.type(input, '2027-01-02T09:00:00Z\ninvalid');
    await user.click(screen.getByRole('button', { name: 'Import dates' }));
    expect(screen.getByText(/Nothing was imported/)).toBeInTheDocument();
    expect(screen.getByTestId('canonical').textContent).toBe(first);
    await user.clear(input);
    await user.type(input, '2027-01-02T09:00:00Z\n2027-01-01T10:00:00+01:00');
    await user.click(screen.getByRole('button', { name: 'Import dates' }));
    expect(screen.getByText('Imported 1 date; skipped 1 duplicate.')).toBeInTheDocument();
    expect(screen.getByTestId('canonical').textContent).toBe(`${first}\n2027-01-02T09:00:00Z`);
  });
  it('requires confirmation to clear all and leaves dates intact when canceled', async () => {
    const user = userEvent.setup();
    render(<Harness initial={first} />);
    await user.click(screen.getByRole('button', { name: 'Clear all' }));
    expect(screen.getByTestId('canonical').textContent).toBe(first);
    await user.click(screen.getByRole('button', { name: 'Keep dates' }));
    expect(screen.getByTestId('canonical').textContent).toBe(first);
    await user.click(screen.getByRole('button', { name: 'Clear all' }));
    await user.click(screen.getByRole('button', { name: 'Confirm clear' }));
    expect(screen.getByText('No included dates yet')).toBeInTheDocument();
  });
  it('keeps active search clearable when removing entries takes the count below the threshold', async () => {
    const user = userEvent.setup();
    const dates = Array.from({ length: 6 }, (_, index) => `2027-01-0${index + 1}T09:00:00Z`);
    render(<Harness initial={dates.join('\n')} />);
    await user.type(screen.getByRole('textbox', { name: 'Search included dates' }), 'Jan 1');
    await user.click(screen.getByRole('button', { name: `Remove ${first}` }));
    expect(screen.getByRole('textbox', { name: 'Search included dates' })).toBeInTheDocument();
    await user.clear(screen.getByRole('textbox', { name: 'Search included dates' }));
    expect(within(screen.getByRole('list', { name: 'Included dates entries' })).getAllByRole('listitem')).toHaveLength(
      5,
    );
  });
  it('supports keyboard row actions and add Enter without submitting the outer form', async () => {
    const user = userEvent.setup();
    const submit = vi.fn((event: { preventDefault: () => void }) => event.preventDefault());
    render(
      <form onSubmit={submit}>
        <Harness initial={first} />
        <button type="submit">Outer submit</button>
      </form>,
    );
    screen.getByRole('button', { name: `Edit ${first}` }).focus();
    await user.keyboard('{Enter}');
    expect(screen.getByLabelText('Edit included date')).toBeInTheDocument();
    await user.keyboard('{Escape}');
    await user.click(screen.getByRole('button', { name: 'Use precise ISO for included dates' }));
    await user.type(screen.getByLabelText('New included date (ISO)'), '2027-01-02T09:00:00Z{Enter}');
    expect(screen.getByText('Date added.')).toBeInTheDocument();
    expect(submit).not.toHaveBeenCalled();
  });
  it('preserves a staged precise date across ISO and picker mode switches', async () => {
    const user = userEvent.setup();
    render(<Harness />);
    await user.click(screen.getByRole('button', { name: 'Use precise ISO for included dates' }));
    await user.type(screen.getByLabelText('New included date (ISO)'), '2027-01-01T10:00:00.000000001+01:00');
    await user.click(screen.getByRole('button', { name: 'Use UTC picker for included dates' }));
    await user.click(screen.getByRole('button', { name: 'Use precise ISO for included dates' }));
    expect(screen.getByLabelText('New included date (ISO)')).toHaveValue('2027-01-01T10:00:00.000000001+01:00');
    await user.click(screen.getByRole('button', { name: 'Add included date' }));
    expect(screen.getByTestId('canonical').textContent).toBe('2027-01-01T10:00:00.000000001+01:00');
  });
});
