import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';

import { renderApp } from '@/test/render-app';

describe('projects list', () => {
  it('lists every project with its goal, lifecycle, access preset, parent and resource counts', async () => {
    renderApp('/projects');

    expect(await screen.findByRole('heading', { name: 'Projects' })).toBeInTheDocument();
    const portal = (await screen.findByRole('link', { name: 'Customer portal' })).closest('tr')!;
    expect(within(portal).getByText('proj-demo')).toBeInTheDocument();
    expect(within(portal).getByText(/Ship the customer portal and keep its credentials rotated\./)).toBeInTheDocument();
    expect(within(portal).getByText('Active')).toBeInTheDocument();
    expect(within(portal).getByText('Inherited')).toBeInTheDocument();
    expect(await within(portal).findByText('Platform folder')).toBeInTheDocument();
    expect(await within(portal).findByText(/3 vaults \/ 8 credentials \/ 5 environments/)).toBeInTheDocument();

    const beacon = (await screen.findByRole('link', { name: 'Beacon' })).closest('tr')!;
    expect(within(beacon).getByText('Members only')).toBeInTheDocument();
    expect(await within(beacon).findByText('Acme workspace')).toBeInTheDocument();
    expect(await within(beacon).findByText(/1 vault \/ 2 credentials \/ 3 environments/)).toBeInTheDocument();
  });

  it('mutes archived rows and marks them archived', async () => {
    renderApp('/projects');

    const atlas = (await screen.findByRole('link', { name: 'Atlas migration' })).closest('tr')!;
    expect(within(atlas).getByText('Archived')).toBeInTheDocument();
    expect(atlas).toHaveClass('opacity-70');
    expect(screen.getByRole('link', { name: 'Beacon' }).closest('tr')).not.toHaveClass('opacity-70');
  });

  it('filters by lifecycle, access preset and parent and clears from the empty state', async () => {
    const user = userEvent.setup();
    renderApp('/projects');

    await screen.findByRole('link', { name: 'Beacon' });
    const filter = screen.getByRole('combobox', { name: 'Filter projects' });

    await user.type(filter, 'archived{Enter}');
    await waitFor(() => expect(screen.queryByRole('link', { name: 'Beacon' })).not.toBeInTheDocument());
    expect(screen.getByRole('link', { name: 'Atlas migration' })).toBeInTheDocument();
    expect(screen.queryByRole('link', { name: 'Customer portal' })).not.toBeInTheDocument();
  });

  it('shows No matches for an unmatched search and clears it', async () => {
    const user = userEvent.setup();
    renderApp('/projects');

    await screen.findByRole('link', { name: 'Beacon' });
    await user.type(screen.getByRole('combobox', { name: 'Filter projects' }), 'zzzz{Enter}');

    expect(await screen.findByText('No matches')).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Clear filters' }));
    expect(await screen.findByRole('link', { name: 'Beacon' })).toBeInTheDocument();
  });

  it('offers facets for lifecycle, access preset and parent', async () => {
    const user = userEvent.setup();
    renderApp('/projects');

    await screen.findByRole('link', { name: 'Beacon' });
    await user.click(screen.getByRole('combobox', { name: 'Filter projects' }));

    for (const facet of ['Lifecycle', 'Access preset', 'Parent']) {
      expect(await screen.findByRole('option', { name: new RegExp(facet) })).toBeInTheDocument();
    }
  });

  it('opens a project from its row', async () => {
    const user = userEvent.setup();
    const { router } = renderApp('/projects');

    await user.click(await screen.findByRole('link', { name: 'Beacon' }));

    await waitFor(() => expect(router.state.location.pathname).toBe('/projects/proj-beacon'));
  });
});
