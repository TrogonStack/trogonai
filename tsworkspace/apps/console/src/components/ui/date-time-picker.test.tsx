import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { describe, expect, it, vi } from 'vitest';

import { DateTimePicker, validDateTimeFields } from './date-time-picker';
import { TimeInput } from './time-input';

function Harness({
  initial = '2027-01-01T00:00:00',
  onChange = () => {},
}: {
  initial?: string;
  onChange?: (value: string) => void;
}) {
  const [value, setValue] = useState(initial);
  return (
    <DateTimePicker
      id="when"
      label="When"
      value={value}
      onChange={(next) => {
        setValue(next);
        onChange(next);
      }}
      zoneLabel="UTC"
    />
  );
}

describe('Base UI date and time controls', () => {
  it('highlights the UTC midnight date, keyboard navigates days and retains seconds on apply', async () => {
    const user = userEvent.setup();
    const change = vi.fn();
    render(<Harness initial="2027-01-01T00:00:17" onChange={change} />);
    await user.click(screen.getByRole('button', { name: 'When' }));
    const selected = screen.getByRole('button', { name: 'Friday, January 1st, 2027, selected' });
    expect(selected).toBeInTheDocument();
    await waitFor(() => expect(selected).toHaveFocus());
    await user.keyboard('{ArrowRight}{Enter}');
    await user.click(screen.getByRole('button', { name: 'Apply date and time' }));
    expect(change).toHaveBeenCalledWith('2027-01-02T00:00:17');
    expect(screen.getByRole('button', { name: 'When' })).toHaveTextContent('2027-01-02');
    await waitFor(() => expect(screen.getByRole('button', { name: 'When' })).toHaveFocus());
  });
  it('stages edits until apply and discards canceled or dismissed changes', async () => {
    const user = userEvent.setup();
    const change = vi.fn();
    render(<Harness onChange={change} />);
    await user.click(screen.getByRole('button', { name: 'When' }));
    await user.click(screen.getByRole('button', { name: 'Saturday, January 2nd, 2027' }));
    await user.click(screen.getByRole('button', { name: 'Cancel' }));
    expect(change).not.toHaveBeenCalled();
    await user.click(screen.getByRole('button', { name: 'When' }));
    expect(screen.getByRole('button', { name: 'Friday, January 1st, 2027, selected' })).toBeInTheDocument();
    await user.keyboard('{Escape}');
    expect(change).not.toHaveBeenCalled();
    await user.click(screen.getByRole('button', { name: 'When' }));
    await user.click(screen.getByRole('button', { name: 'Apply date and time' }));
    expect(change).not.toHaveBeenCalled();
  });
  it('preserves minute-only field spelling when opening and applying unchanged', async () => {
    const user = userEvent.setup();
    const change = vi.fn();
    render(<Harness initial="2027-01-01T09:00" onChange={change} />);
    await user.click(screen.getByRole('button', { name: 'When' }));
    await user.click(screen.getByRole('button', { name: 'Apply date and time' }));
    expect(change).not.toHaveBeenCalled();
  });
  it('disables apply with missing or out-of-range Base NumberField time segments', async () => {
    const user = userEvent.setup();
    render(<Harness />);
    await user.click(screen.getByRole('button', { name: 'When' }));
    const hours = screen.getByRole('textbox', { name: 'When time hours' });
    await user.clear(hours);
    expect(screen.getByRole('button', { name: 'Apply date and time' })).toBeDisabled();
    await user.type(hours, '25');
    expect(screen.getByRole('button', { name: 'Apply date and time' })).toBeDisabled();
    await user.clear(hours);
    await user.type(hours, '23');
    expect(screen.getByRole('button', { name: 'Apply date and time' })).toBeEnabled();
  });
  it('lets DayPicker own month navigation and selection', async () => {
    const user = userEvent.setup();
    render(<Harness />);
    await user.click(screen.getByRole('button', { name: 'When' }));
    await user.click(screen.getByRole('button', { name: 'Go to the Next Month' }));
    expect(screen.getByRole('grid', { name: 'February 2027' })).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Monday, February 1st, 2027' }));
    await user.click(screen.getByRole('button', { name: 'Apply date and time' }));
    expect(screen.getByRole('button', { name: 'When' })).toHaveTextContent('2027-02-01');
  });
  it('has a strict whole-second local-fields contract and supports years below100', async () => {
    expect(validDateTimeFields('2027-01-01T00:00:00.123')).toBe(false);
    expect(validDateTimeFields('2027-01-01T00:00:00Z')).toBe(false);
    expect(validDateTimeFields('2027-02-30T00:00:00')).toBe(false);
    const user = userEvent.setup();
    render(<Harness initial="0099-01-01T00:00:00" />);
    await user.click(screen.getByRole('button', { name: 'When' }));
    expect(screen.getByRole('button', { name: 'Thursday, January 1st, 99, selected' })).toBeInTheDocument();
  });
  it('keeps incomplete inline time editable rather than retaining an old valid time', async () => {
    const user = userEvent.setup();
    const changes = vi.fn();
    function TimeHarness() {
      const [value, setValue] = useState('09:30');
      return (
        <>
          <TimeInput
            id="clock"
            label="Clock"
            value={value}
            onChange={(next) => {
              setValue(next);
              changes(next);
            }}
          />
          <output>{value}</output>
        </>
      );
    }
    render(<TimeHarness />);
    const hours = screen.getByRole('textbox', { name: 'Clock hours' });
    await user.clear(hours);
    expect(changes).toHaveBeenLastCalledWith(':30');
    await user.type(hours, '10');
    expect(changes).toHaveBeenLastCalledWith('10:30');
    await user.keyboard('{ArrowUp}');
    expect(changes).toHaveBeenLastCalledWith('11:30');
  });
});
