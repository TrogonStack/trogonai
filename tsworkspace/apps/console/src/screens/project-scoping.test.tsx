import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';

import { renderApp } from '@/test/render-app';

async function switchTo(user: ReturnType<typeof userEvent.setup>, from: string, to: string) {
  await user.click(await screen.findByRole('button', { name: `Project: ${from}. Switch project` }));
  const list = await screen.findByRole('list', { name: 'Projects' });
  await user.click(within(list).getByRole('button', { name: new RegExp(to) }));
}

describe('project scoping', () => {
  it('defaults to the first active project when the search param is missing', async () => {
    renderApp('/secrets');

    expect(await screen.findByRole('link', { name: 'OpenAI production key' })).toBeInTheDocument();
    expect(await screen.findByRole('button', { name: 'Project: Customer portal. Switch project' })).toBeInTheDocument();
    expect(screen.queryByRole('link', { name: 'SendGrid delivery key' })).not.toBeInTheDocument();
  });

  it('defaults to the first active project when the search param is unknown', async () => {
    renderApp('/secrets?project=proj-nowhere');

    expect(await screen.findByRole('link', { name: 'OpenAI production key' })).toBeInTheDocument();
    expect(await screen.findByRole('button', { name: 'Project: Customer portal. Switch project' })).toBeInTheDocument();
  });

  it('shows the credentials of the project in the search param', async () => {
    renderApp('/secrets?project=proj-beacon');

    expect(await screen.findByRole('link', { name: 'SendGrid delivery key' })).toBeInTheDocument();
    expect(screen.getByRole('link', { name: 'Sentry ingest token' })).toBeInTheDocument();
    expect(screen.queryByRole('link', { name: 'OpenAI production key' })).not.toBeInTheDocument();
  });

  it('lists the switcher with active projects first, archived marked and the shortcuts', async () => {
    const user = userEvent.setup();
    renderApp('/projects');

    await user.click(await screen.findByRole('button', { name: 'Project: Customer portal. Switch project' }));
    const list = await screen.findByRole('list', { name: 'Projects' });
    const names = within(list)
      .getAllByRole('button')
      .map((button) => button.textContent);
    expect(names[0]).toMatch(/Customer portal/);
    expect(names[names.length - 1]).toMatch(/Atlas migration.*Archived/);
    const popover = list.parentElement!.parentElement!;
    expect(within(popover).getByRole('link', { name: 'All projects' })).toBeInTheDocument();
    expect(within(popover).getByRole('link', { name: 'New project' })).toBeInTheDocument();
  });

  it('switches the secrets to the chosen project', async () => {
    const user = userEvent.setup();
    const { router } = renderApp('/secrets');

    await screen.findByRole('link', { name: 'OpenAI production key' });
    await switchTo(user, 'Customer portal', 'Beacon');

    expect(await screen.findByRole('link', { name: 'SendGrid delivery key' })).toBeInTheDocument();
    expect(screen.queryByRole('link', { name: 'OpenAI production key' })).not.toBeInTheDocument();
    expect(router.state.location.pathname).toBe('/secrets');
    expect(router.state.location.search).toMatchObject({ project: 'proj-beacon' });
  });

  it('switches the environment registry to the chosen project', async () => {
    const user = userEvent.setup();
    renderApp('/environments');

    expect(await screen.findByRole('link', { name: 'QA' })).toBeInTheDocument();
    await switchTo(user, 'Customer portal', 'Beacon');

    await waitFor(() => expect(screen.queryByRole('link', { name: 'QA' })).not.toBeInTheDocument());
    expect(screen.queryByRole('link', { name: 'Staging' })).not.toBeInTheDocument();
    for (const name of ['Production', 'Preview', 'Development']) {
      expect(await screen.findByRole('link', { name })).toBeInTheDocument();
    }
  });

  it('keeps the project in the search param across navigation', async () => {
    const user = userEvent.setup();
    const { router } = renderApp('/secrets?project=proj-beacon');

    await screen.findByRole('link', { name: 'SendGrid delivery key' });
    const nav = screen.getByRole('navigation', { name: 'Primary' });

    await user.click(within(nav).getByRole('link', { name: /Environments/ }));
    await waitFor(() => expect(router.state.location.pathname).toBe('/environments'));
    expect(router.state.location.search).toMatchObject({ project: 'proj-beacon' });

    await user.click(within(nav).getByRole('link', { name: /Projects/ }));
    await waitFor(() => expect(router.state.location.pathname).toBe('/projects'));
    expect(router.state.location.search).toMatchObject({ project: 'proj-beacon' });

    await user.click(within(nav).getByRole('link', { name: /Workspaces/ }));
    await waitFor(() => expect(router.state.location.pathname).toBe('/workspaces'));
    expect(router.state.location.search).toMatchObject({ project: 'proj-beacon' });

    await user.click(within(nav).getByRole('link', { name: /Secrets/ }));
    await waitFor(() => expect(router.state.location.pathname).toBe('/secrets'));
    expect(router.state.location.search).toMatchObject({ project: 'proj-beacon' });
    expect(await screen.findByRole('link', { name: 'SendGrid delivery key' })).toBeInTheDocument();
  });

  it('opens the secrets of a project from its detail page', async () => {
    const user = userEvent.setup();
    const { router } = renderApp('/projects/proj-beacon');

    await user.click(await screen.findByRole('link', { name: 'Open secrets' }));

    await waitFor(() => expect(router.state.location.pathname).toBe('/secrets'));
    expect(await screen.findByRole('link', { name: 'SendGrid delivery key' })).toBeInTheDocument();
  });

  it('starts the breadcrumbs with the current project on scoped pages', async () => {
    renderApp('/environments?project=proj-beacon');

    const crumbs = await screen.findByRole('navigation', { name: 'Breadcrumb' });
    expect(await within(crumbs).findByText('Beacon')).toBeInTheDocument();
    expect(within(crumbs).getByText('Environments')).toBeInTheDocument();
  });

  it('keeps provider connections independent of the project', async () => {
    renderApp('/providers?project=proj-beacon');

    const table = await screen.findByRole('table');
    expect(await within(table).findByText('OpenAI production')).toBeInTheDocument();
  });
});
