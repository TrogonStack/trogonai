import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { describe, expect, it, vi } from 'vitest';

import { CronBuilder } from './cron-builder';
import { RecurrenceBuilder } from './recurrence-builder';
import { ScheduleForm } from './schedule-form';
import { InstantField, IntervalField } from './timing-fields';

function CronHarness({ initial = '0 0 9 * * MON-FRI' }: { initial?: string }) {
  const [value, onChange] = useState(initial);
  return <CronBuilder value={value} onChange={onChange} timezone="America/New_York" />;
}
function RuleHarness({ initial = 'FREQ=WEEKLY;BYDAY=MO,WE,FR' }: { initial?: string }) {
  const [value, onChange] = useState(initial);
  return <RecurrenceBuilder value={value} onChange={onChange} />;
}

describe('guided schedule controls', () => {
  it('edits a cron preset, including clearing and retyping a numeric field', async () => {
    const user = userEvent.setup();
    render(<CronHarness />);
    await user.selectOptions(screen.getByLabelText('Pattern'), 'minutes');
    const amount = screen.getByLabelText('Every how many minutes');
    await user.clear(amount);
    expect(amount).toBeInTheDocument();
    await user.type(amount, '7');
    expect(screen.getByLabelText('Cron expression')).toHaveValue('0 */7 * * * *');
    expect(screen.getByText(/restarting each hour/)).toBeInTheDocument();
  });
  it('preserves advanced cron when switching modes and only replaces explicitly', async () => {
    const user = userEvent.setup();
    const raw = '17 10,40 9 * * * 2027';
    render(<CronHarness initial={raw} />);
    expect(screen.getByLabelText('Cron expression')).toHaveValue(raw);
    await user.click(screen.getByRole('button', { name: 'Builder' }));
    expect(screen.getByText(/preserved exactly/)).toBeInTheDocument();
    expect(screen.queryByLabelText('Pattern')).not.toBeInTheDocument();
    expect(screen.getByText(/Custom cron expression/)).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Expression' }));
    expect(screen.getByLabelText('Cron expression')).toHaveValue(raw);
    await user.click(screen.getByRole('button', { name: 'Builder' }));
    await user.click(screen.getByRole('button', { name: 'Replace with starter pattern' }));
    expect(screen.getByLabelText('Cron expression')).toHaveValue('0 0 9 * * MON-FRI');
  });
  it('matches initial weekdays and builds an ordinal recurrence with an end condition', async () => {
    const user = userEvent.setup();
    render(<RuleHarness />);
    expect(screen.getByRole('button', { name: 'Monday' })).toHaveAttribute('aria-pressed', 'true');
    expect(screen.getByRole('button', { name: 'Tuesday' })).toHaveAttribute('aria-pressed', 'false');
    await user.selectOptions(screen.getByLabelText('Frequency'), 'MONTHLY');
    await user.selectOptions(screen.getByLabelText('Repeat by'), 'ordinal');
    await user.selectOptions(screen.getByLabelText('Position'), '-1');
    await user.selectOptions(screen.getByLabelText('Weekday'), 'FR');
    await user.selectOptions(screen.getByLabelText('Ends'), 'count');
    await user.clear(screen.getByLabelText('Occurrence count'));
    expect(screen.getByLabelText('Occurrence count')).toBeInTheDocument();
    await user.type(screen.getByLabelText('Occurrence count'), '6');
    expect(screen.getByLabelText('Recurrence rule')).toHaveValue('FREQ=MONTHLY;BYDAY=-1FR;COUNT=6');
    await user.selectOptions(screen.getByLabelText('Ends'), 'until');
    expect((screen.getByLabelText('Recurrence rule') as HTMLInputElement).value).not.toContain('COUNT=');
  });
  it('retains unknown RRULE clauses byte for byte on mode switches', async () => {
    const user = userEvent.setup();
    const raw = 'FREQ=WEEKLY;WKST=SU;BYDAY=MO';
    render(<RuleHarness initial={raw} />);
    await user.click(screen.getByRole('button', { name: 'Builder' }));
    expect(screen.getByText(/preserved exactly/)).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Expression' }));
    expect(screen.getByLabelText('Recurrence rule')).toHaveValue(raw);
  });
  it('does not rewrite supported expressions merely by switching editor modes', async () => {
    const user = userEvent.setup();
    const raw = 'BYDAY=MO,WE,FR;FREQ=WEEKLY;INTERVAL=1';
    render(<RuleHarness initial={raw} />);
    await user.click(screen.getByRole('button', { name: 'Expression' }));
    expect(screen.getByLabelText('Recurrence rule')).toHaveValue(raw);
    await user.click(screen.getByRole('button', { name: 'Builder' }));
    expect(screen.getByLabelText('Recurrence rule')).toHaveValue(raw);
  });
  it('changes interval display units without rewriting duration', async () => {
    const user = userEvent.setup();
    function Harness() {
      const [value, onChange] = useState('300');
      return <IntervalField value={value} onChange={onChange} />;
    }
    render(<Harness />);
    expect(screen.getByLabelText('Repeat every')).toHaveValue(5);
    await user.selectOptions(screen.getByLabelText('Interval unit'), '1');
    expect(screen.getByLabelText('Repeat every')).toHaveValue(300);
    await user.clear(screen.getByLabelText('Repeat every'));
    await user.type(screen.getByLabelText('Repeat every'), '90');
    expect(screen.getByText(/90 seconds between fires/)).toBeInTheDocument();
  });
  it('preserves precise ISO timestamps when switching date input modes', async () => {
    const user = userEvent.setup();
    const change = vi.fn();
    render(<InstantField id="test-instant" label="Instant" value="2027-01-01T09:00:00.123456789Z" onChange={change} />);
    await user.click(screen.getByRole('button', { name: 'Use UTC date picker' }));
    await user.click(screen.getByRole('button', { name: 'Instant' }));
    await user.click(screen.getByRole('button', { name: 'Cancel' }));
    await user.click(screen.getByRole('button', { name: 'Use precise ISO input' }));
    expect(screen.getByLabelText('Instant')).toHaveValue('2027-01-01T09:00:00.123456789Z');
    expect(change).not.toHaveBeenCalled();
  });
  it('submits guided rules and date exceptions through the canonical form draft', async () => {
    const user = userEvent.setup();
    const submit = vi.fn().mockResolvedValue(undefined);
    render(<ScheduleForm pending={false} onSubmit={submit} />);
    await user.click(screen.getByRole('radio', { name: /Recurrence rule/ }));
    await user.click(screen.getByRole('button', { name: 'Use precise ISO input' }));
    await user.type(screen.getByLabelText('Recurrence start'), '2027-01-01T09:00:00Z');
    await user.selectOptions(screen.getByLabelText('Frequency'), 'DAILY');
    await user.selectOptions(screen.getByLabelText('Ends'), 'count');
    await user.clear(screen.getByLabelText('Occurrence count'));
    await user.type(screen.getByLabelText('Occurrence count'), '4');
    await user.click(screen.getByText('Include or exclude specific dates'));
    await user.click(screen.getByRole('button', { name: 'Use precise ISO for included dates' }));
    await user.type(screen.getByLabelText('New included date (ISO)'), '2027-01-08T09:00:00Z');
    await user.click(screen.getByRole('button', { name: 'Add included date' }));
    await user.click(screen.getByRole('button', { name: 'Use precise ISO for excluded dates' }));
    await user.type(screen.getByLabelText('New excluded date (ISO)'), '2027-01-08T10:00:00+01:00');
    await user.click(screen.getByRole('button', { name: 'Add excluded date' }));
    expect(screen.getByText('Also excluded. This occurrence will be skipped.')).toBeInTheDocument();
    expect(screen.getByText('Also included. Exclusion takes precedence.')).toBeInTheDocument();
    await user.type(screen.getByLabelText('Destination subject'), 'jobs.guided');
    await user.click(screen.getByRole('button', { name: 'Create schedule' }));
    expect(submit).toHaveBeenCalledWith(
      expect.objectContaining({
        rrule: 'FREQ=DAILY;COUNT=4',
        rdate: '2027-01-08T09:00:00Z',
        exdate: '2027-01-08T10:00:00+01:00',
        kind: 'rrule',
      }),
    );
  });
  it('blocks invalid guided intervals from submitting', async () => {
    const user = userEvent.setup();
    const submit = vi.fn();
    render(<ScheduleForm pending={false} onSubmit={submit} />);
    await user.click(screen.getByRole('radio', { name: /Cron/ }));
    await user.selectOptions(screen.getByLabelText('Pattern'), 'minutes');
    await user.clear(screen.getByLabelText('Every how many minutes'));
    await user.type(screen.getByLabelText('Every how many minutes'), '99');
    await user.type(screen.getByLabelText('Destination subject'), 'jobs.invalid');
    await user.click(screen.getByRole('button', { name: 'Create schedule' }));
    expect(submit).not.toHaveBeenCalled();
    expect(screen.getByText(/within their ranges/)).toBeInTheDocument();
  });
});
