import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';

import { renderApp } from '@/test/render-app';

describe('sources overview', () => {
  it('lists the seeded integrations, summary and preview notice', async () => {
    renderApp('/sources');

    expect(await screen.findByRole('heading', { name: 'Sources' })).toBeInTheDocument();
    expect(screen.getByText('Preview')).toBeInTheDocument();
    expect(await screen.findByRole('link', { name: 'GitHub local' })).toBeInTheDocument();
    expect(screen.getByRole('link', { name: 'Telegram support bot' })).toBeInTheDocument();
    expect(screen.getByRole('link', { name: 'Sentry production' })).toBeInTheDocument();

    const summary = screen.getByRole('group', { name: 'Sources summary' });
    for (const label of ['Integrations', 'Receiving', 'Needs attention', 'Rejected signatures']) {
      expect(within(summary).getByText(label)).toBeInTheDocument();
    }
  });

  it('shows custody tiers by product name', async () => {
    renderApp('/sources');

    await screen.findByRole('link', { name: 'GitHub local' });
    expect(screen.getAllByText('Managed key').length).toBeGreaterThan(0);
    expect(screen.getAllByText('Dedicated key').length).toBeGreaterThan(0);
    expect(screen.getAllByText('Customer managed key').length).toBeGreaterThan(0);
    expect(screen.getByText('Key custody tiers')).toBeInTheDocument();
  });

  it('filters by status through the filter bar', async () => {
    const user = userEvent.setup();
    renderApp('/sources');

    await screen.findByRole('link', { name: 'GitHub local' });
    await user.click(screen.getByRole('combobox', { name: 'Filter sources' }));
    await user.click(await screen.findByRole('option', { name: 'Status' }));
    await user.click(await screen.findByRole('option', { name: /Paused/ }));

    expect(screen.queryByRole('link', { name: 'GitHub local' })).not.toBeInTheDocument();
    expect(screen.getByRole('link', { name: 'Linear product' })).toBeInTheDocument();
  });

  it('opens an integration from the table', async () => {
    const user = userEvent.setup();
    renderApp('/sources');

    await user.click(await screen.findByRole('link', { name: 'GitHub local' }));

    expect(await screen.findByRole('heading', { name: 'GitHub local' })).toBeInTheDocument();
  });
});


it('returns to the source list when switching projects from a detail', async () => {
  const user = userEvent.setup();
  const { router } = renderApp('/sources/github%2Flocal');
  await screen.findByRole('heading', { name: 'GitHub local' });
  await user.click(screen.getByRole('button', { name: /Switch project/ }));
  await user.click(await screen.findByRole('button', { name: 'Beacon' }));
  await waitFor(() => expect(router.state.location.pathname).toBe('/sources'));
  expect(router.state.location.search).toMatchObject({ project: 'proj-beacon' });
});
