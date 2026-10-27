import { screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';

import { seedIds } from '@/features/secrets/fake/seed';
import { renderApp } from '@/test/render-app';

describe('vault detail', () => {
  it('lists only the credentials in the vault and fixes the environment', async () => {
    renderApp(`/secrets/vaults/${seedIds.vaults.webhooks}`);

    expect(await screen.findByRole('heading', { name: 'Webhooks' })).toBeInTheDocument();
    expect(await screen.findByRole('link', { name: 'GitHub webhook verification' })).toBeInTheDocument();
    expect(screen.queryByRole('link', { name: 'OpenAI production key' })).not.toBeInTheDocument();
    expect(screen.getByText('Immutable environment')).toBeInTheDocument();
  });

  it('blocks vault destruction while live credentials remain and says why', async () => {
    renderApp(`/secrets/vaults/${seedIds.vaults.webhooks}`);

    const button = await screen.findByRole('button', { name: 'Schedule destruction' });
    expect(button).toBeDisabled();
    expect(await screen.findByText(/credential/i, { selector: 'p.text-right' })).toBeInTheDocument();
  });

  it('disables and re-enables a vault', async () => {
    const user = userEvent.setup();
    renderApp(`/secrets/vaults/${seedIds.vaults.staging}`);

    await user.click(await screen.findByRole('button', { name: 'Disable' }));
    await user.click(await screen.findByRole('button', { name: 'Enable' }));

    await waitFor(() => expect(screen.getByRole('button', { name: 'Disable' })).toBeInTheDocument());
  });
});
