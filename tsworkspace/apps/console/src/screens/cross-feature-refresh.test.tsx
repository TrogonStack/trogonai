import { QueryObserver } from '@tanstack/react-query';
import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';

import { createFakeBackend } from '@/fake-backend';
import { environmentKeys } from '@/features/environments/queries';
import { seedProjectIds } from '@/features/projects/fake/seed';
import { projectKeys } from '@/features/projects/queries';
import { renderApp } from '@/test/render-app';

const demo = seedProjectIds.demo;

describe('cross-feature refresh', () => {
  it('refreshes the project resource counts after an environment is created', async () => {
    const user = userEvent.setup();
    const backend = createFakeBackend({ latencyMs: 0 });
    const { queryClient } = renderApp('/environments', backend);
    const counts = new QueryObserver(queryClient, {
      queryKey: projectKeys.resources(demo),
      queryFn: () => backend.projects.resources(demo),
    });
    const unsubscribe = counts.subscribe(() => undefined);
    await waitFor(() => expect(counts.getCurrentResult().data).toBeDefined());
    const before = counts.getCurrentResult().data!.environments;

    await user.click(await screen.findByRole('button', { name: 'New environment' }));
    const dialog = await screen.findByRole('dialog');
    await user.type(within(dialog).getByLabelText('Environment name'), 'Load testing');
    await user.click(within(dialog).getByRole('button', { name: 'Create environment' }));

    await waitFor(() => expect(counts.getCurrentResult().data!.environments).toBe(before + 1));
    unsubscribe();
  });

  it('refreshes the project resource counts and the environments after a vault is created', async () => {
    const user = userEvent.setup();
    const backend = createFakeBackend({ latencyMs: 0 });
    const { queryClient } = renderApp('/secrets', backend);
    const counts = new QueryObserver(queryClient, {
      queryKey: projectKeys.resources(demo),
      queryFn: () => backend.projects.resources(demo),
    });
    const environments = new QueryObserver(queryClient, {
      queryKey: environmentKeys.list(demo),
      queryFn: () => backend.environments.forProject(demo).list(),
    });
    const unsubscribeCounts = counts.subscribe(() => undefined);
    const unsubscribeEnvironments = environments.subscribe(() => undefined);
    await waitFor(() => expect(counts.getCurrentResult().data).toBeDefined());
    await waitFor(() => expect(environments.getCurrentResult().data).toBeDefined());
    const before = counts.getCurrentResult().data!.vaults;
    const environmentFetches = environments.getCurrentResult().dataUpdatedAt;

    await user.click(await screen.findByRole('button', { name: 'New vault' }));
    const dialog = await screen.findByRole('dialog');
    await user.type(within(dialog).getByLabelText('Vault name'), 'Regression');
    const select = await within(dialog).findByRole('combobox', { name: 'Environment' });
    await within(select).findByRole('option', { name: 'QA' });
    await user.selectOptions(select, 'QA');
    await user.click(within(dialog).getByRole('button', { name: 'Create vault' }));

    await waitFor(() => expect(counts.getCurrentResult().data!.vaults).toBe(before + 1));
    await waitFor(() =>
      expect(queryClient.getQueryState(environmentKeys.list(demo))!.dataUpdatedAt).toBeGreaterThan(environmentFetches),
    );
    unsubscribeCounts();
    unsubscribeEnvironments();
  });
});
