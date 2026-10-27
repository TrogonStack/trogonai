import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';

import { createFakeBackend } from '@/fake-backend';
import { seedProjectIds } from '@/features/projects/fake/seed';
import { renderApp } from '@/test/render-app';

describe('environments list', () => {
  it('lists the built-in and custom environments with their usage', async () => {
    renderApp('/environments');

    expect(await screen.findByRole('heading', { name: 'Environments' })).toBeInTheDocument();
    for (const name of ['Production', 'Preview', 'Development', 'Staging', 'QA']) {
      expect(await screen.findByRole('link', { name })).toBeInTheDocument();
    }
    expect(screen.getByRole('group', { name: 'Environment summary' })).toBeInTheDocument();
    expect(screen.getAllByText('Built-in').length).toBeGreaterThan(0);
    expect(screen.getAllByText('Custom').length).toBeGreaterThan(0);
  });

  it('shows the Environments entry in the sidebar', async () => {
    renderApp('/environments');

    const nav = await screen.findByRole('navigation', { name: 'Primary' });
    expect(within(nav).getByRole('link', { name: /Environments/ })).toBeInTheDocument();
  });

  it('filters by kind and clears the filter from the empty state', async () => {
    const user = userEvent.setup();
    renderApp('/environments');

    await screen.findByRole('link', { name: 'QA' });
    await user.type(screen.getByRole('combobox', { name: 'Filter environments' }), 'zzzz{Enter}');

    expect(await screen.findByText('No matches')).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Clear filters' }));
    expect(await screen.findByRole('link', { name: 'QA' })).toBeInTheDocument();
  });

  it('opens an environment from its row', async () => {
    const user = userEvent.setup();
    const { router } = renderApp('/environments');

    await user.click(await screen.findByRole('link', { name: 'QA' }));

    await waitFor(() => expect(router.state.location.pathname).toBe('/environments/qa'));
  });

  it('creates an environment from the dialog', async () => {
    const user = userEvent.setup();
    renderApp('/environments');

    await user.click(await screen.findByRole('button', { name: 'New environment' }));
    const dialog = await screen.findByRole('dialog');
    await user.type(within(dialog).getByLabelText('Environment name'), 'Load testing');
    expect(within(dialog).getByLabelText('Environment id')).toHaveValue('load-testing');
    await user.click(within(dialog).getByRole('button', { name: 'Create environment' }));

    expect(await screen.findByRole('link', { name: 'Load testing' })).toBeInTheDocument();
  });

  it('rejects an invalid id and a duplicate id', async () => {
    const user = userEvent.setup();
    renderApp('/environments');

    await user.click(await screen.findByRole('button', { name: 'New environment' }));
    const dialog = await screen.findByRole('dialog');
    await user.type(within(dialog).getByLabelText('Environment name'), 'Odd one');
    const id = within(dialog).getByLabelText('Environment id');
    await user.clear(id);
    await user.type(id, 'Not Valid');
    await user.click(within(dialog).getByRole('button', { name: 'Create environment' }));
    expect(await within(dialog).findByText('An environment id starts with a lowercase letter.')).toBeInTheDocument();

    await user.clear(id);
    await user.type(id, 'qa');
    await user.click(within(dialog).getByRole('button', { name: 'Create environment' }));
    expect(await within(dialog).findByRole('alert')).toHaveTextContent(/already/i);
  });
});

describe('environment detail', () => {
  it('shows a built-in environment with its vaults and credentials and blocks delete', async () => {
    renderApp('/environments/production');

    expect(await screen.findByRole('heading', { name: 'Production' })).toBeInTheDocument();
    expect(await screen.findByRole('link', { name: 'OpenAI production key' })).toBeInTheDocument();
    expect(screen.queryByRole('link', { name: 'Gemini staging key' })).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Delete' })).toBeDisabled();
    expect(screen.getByText('A built-in environment cannot be deleted.')).toBeInTheDocument();
  });

  it('blocks delete with a reason while a vault uses the environment', async () => {
    renderApp('/environments/staging');

    expect(await screen.findByRole('heading', { name: 'Staging' })).toBeInTheDocument();
    expect(await screen.findByRole('button', { name: 'Delete' })).toBeDisabled();
    expect(screen.getByText(/1 vault uses this environment/)).toBeInTheDocument();
  });

  it('deletes an unused custom environment after confirmation', async () => {
    const user = userEvent.setup();
    const { router } = renderApp('/environments/qa');

    expect(await screen.findByText(/No vault lives here yet/)).toBeInTheDocument();
    await user.click(await screen.findByRole('button', { name: 'Delete' }));
    await user.click(await screen.findByRole('button', { name: 'Delete environment' }));

    await waitFor(() => expect(router.state.location.pathname).toBe('/environments'));
    await waitFor(() => expect(screen.queryByRole('link', { name: 'QA' })).not.toBeInTheDocument());
  });

  it('edits the name and description but not the id', async () => {
    const user = userEvent.setup();
    renderApp('/environments/qa');

    await user.click(await screen.findByRole('button', { name: 'Edit' }));
    const dialog = await screen.findByRole('dialog');
    expect(within(dialog).getByLabelText('Environment id')).toBeDisabled();
    const name = within(dialog).getByLabelText('Environment name');
    await user.clear(name);
    await user.type(name, 'Quality');
    await user.click(within(dialog).getByRole('button', { name: 'Save changes' }));

    expect(await screen.findByRole('heading', { name: 'Quality' })).toBeInTheDocument();
  });

  it('says so when the environment does not exist', async () => {
    renderApp('/environments/nowhere');

    expect(await screen.findByText('Environment not found')).toBeInTheDocument();
  });

  it('links a vault environment chip to the environment', async () => {
    const user = userEvent.setup();
    const { router } = renderApp('/secrets/vaults/vlt_staging');

    await user.click(await screen.findByRole('link', { name: 'Staging' }));

    await waitFor(() => expect(router.state.location.pathname).toBe('/environments/staging'));
  });
});

describe('vault creation and environments', () => {
  it('offers the registry environments, including custom ones', async () => {
    const user = userEvent.setup();
    const backend = createFakeBackend({ latencyMs: 0 });
    renderApp('/secrets', backend);

    await user.click(await screen.findByRole('button', { name: 'New vault' }));
    const dialog = await screen.findByRole('dialog');
    const select = await within(dialog).findByRole('combobox', { name: 'Environment' });

    for (const name of ['Production', 'Preview', 'Development', 'Staging', 'QA']) {
      expect(await within(select).findByRole('option', { name })).toBeInTheDocument();
    }
  });

  it('creates a vault in a custom environment', async () => {
    const user = userEvent.setup();
    const backend = createFakeBackend({ latencyMs: 0 });
    renderApp('/secrets', backend);

    await user.click(await screen.findByRole('button', { name: 'New vault' }));
    const dialog = await screen.findByRole('dialog');
    await user.type(within(dialog).getByLabelText('Vault name'), 'Regression');
    const select = await within(dialog).findByRole('combobox', { name: 'Environment' });
    await within(select).findByRole('option', { name: 'QA' });
    await user.selectOptions(select, 'QA');
    await user.click(within(dialog).getByRole('button', { name: 'Create vault' }));

    await waitFor(async () => {
      const vaults = await backend.secrets.forProject(seedProjectIds.demo).listVaults();
      expect(vaults.find((vault) => vault.displayName === 'Regression')?.environment).toBe('qa');
    });
  });
});
