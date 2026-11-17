import { act, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';

import { renderApp } from '@/test/render-app';

const first = '00000000-0000-4000-8000-000000000001';
const paused = '00000000-0000-4000-8000-000000000003';

describe('fake scheduling GUI', () => {
  it('lists all strategies and filters by state, strategy and subject', async () => {
    const user = userEvent.setup();
    renderApp('/schedules');
    await screen.findByRole('link', { name: first });
    expect(screen.getByRole('heading', { name: 'Schedules' })).toBeInTheDocument();
    expect(within(screen.getByRole('group', { name: 'Schedules summary' })).getByText('Completed')).toBeInTheDocument();
    await user.selectOptions(screen.getByLabelText('Filter schedule status'), 'paused');
    expect(screen.getByRole('link', { name: paused })).toBeInTheDocument();
    expect(screen.queryByRole('link', { name: first })).not.toBeInTheDocument();
    await user.selectOptions(screen.getByLabelText('Filter schedule status'), 'all');
    await user.selectOptions(screen.getByLabelText('Filter schedule strategy'), 'cron');
    expect(screen.getByText('reports.daily')).toBeInTheDocument();
    await user.type(screen.getByLabelText('Search schedules'), 'missing');
    expect(screen.getByText('No matching schedules')).toBeInTheDocument();
  });
  it('pauses and resumes, then requires confirmation to remove', async () => {
    const user = userEvent.setup();
    renderApp(`/schedules/${first}`);
    await user.click(await screen.findByRole('button', { name: 'Pause schedule' }));
    await user.click(await screen.findByRole('button', { name: 'Resume schedule' }));
    await screen.findByRole('button', { name: 'Pause schedule' });
    await user.click(screen.getByRole('button', { name: 'Remove schedule' }));
    const dialog = await screen.findByRole('alertdialog');
    await user.click(within(dialog).getByRole('button', { name: 'Cancel' }));
    expect(screen.getByRole('heading', { name: 'ops.metrics.refresh' })).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Remove schedule' }));
    await user.click(within(await screen.findByRole('alertdialog')).getByRole('button', { name: 'Remove schedule' }));
    await screen.findByRole('heading', { name: 'Schedules' });
    expect(screen.queryByRole('link', { name: first })).not.toBeInTheDocument();
  });
  it('creates a paused interval with delivery and static content', async () => {
    const user = userEvent.setup();
    await act(async () => {
      await renderApp('/schedules/new').router.load();
    });
    const subject = await screen.findByLabelText('Destination subject');
    await user.type(subject, 'jobs.test.created');
    await user.selectOptions(screen.getByLabelText('Initial status'), 'paused');
    await user.click(screen.getByRole('button', { name: 'Create schedule' }));
    await screen.findByRole('heading', { name: 'jobs.test.created' });
    expect(screen.getByRole('button', { name: 'Resume schedule' })).toBeInTheDocument();
    expect(screen.getByText('Every 300 seconds')).toBeInTheDocument();
    expect(screen.getByText('application/json')).toBeInTheDocument();
  });
  it('renders every strategy editor and validates required fields', async () => {
    const user = userEvent.setup();
    renderApp('/schedules/new');
    await user.click(await screen.findByRole('radio', { name: /Once/ }));
    expect(screen.getByLabelText('Fire at')).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Create schedule' }));
    expect(await screen.findByText('Use an ISO 8601 instant with Z or a UTC offset.')).toBeInTheDocument();
    await user.click(screen.getByRole('radio', { name: /Cron/ }));
    expect(screen.getByLabelText('Cron expression')).toHaveValue('0 0 9 * * MON-FRI');
    await user.click(screen.getByRole('radio', { name: /Recurrence rule/ }));
    expect(screen.getByRole('region', { name: 'Included dates' })).toBeInTheDocument();
    expect(screen.getByRole('region', { name: 'Excluded dates' })).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Add header' }));
    expect(screen.getByLabelText('Header 1 name')).toBeInTheDocument();
  });
  it('returns to the list on project switch and does not leak schedules', async () => {
    const user = userEvent.setup();
    const { router } = renderApp(`/schedules/${first}`);
    await screen.findByRole('heading', { name: 'ops.metrics.refresh' });
    await user.click(screen.getByRole('button', { name: /Switch project/ }));
    await user.click(await screen.findByRole('button', { name: 'Beacon' }));
    await waitFor(() => expect(router.state.location.pathname).toBe('/schedules'));
    expect(await screen.findByText('No schedules yet')).toBeInTheDocument();
  });
});
