import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';

import { seedIntegrationIds } from '@/features/gateway-sources/fake/seed';
import { seedIds } from '@/features/secrets/fake/seed';
import { renderApp } from '@/test/render-app';

const path = (id: string) => `/sources/${encodeURIComponent(id)}`;

describe('source detail', () => {
  it('decodes the slash in the integration id and shows the panels', async () => {
    renderApp(path(seedIntegrationIds.github));

    expect(await screen.findByRole('heading', { name: 'GitHub local' })).toBeInTheDocument();
    expect(screen.getByText('github/local')).toBeInTheDocument();
    expect(screen.getByText('trogon.github.local')).toBeInTheDocument();
    expect(screen.getByText('Credential slots')).toBeInTheDocument();
    expect(screen.getByText('Webhook registration')).toBeInTheDocument();
    expect(screen.getByText('Key custody')).toBeInTheDocument();
    expect(screen.getByText('Managed by the platform')).toBeInTheDocument();
    expect(screen.getByText('Delivery activity')).toBeInTheDocument();
    expect(screen.getByRole('list', { name: 'Recent deliveries' })).toBeInTheDocument();
  });

  it('links a slot to the credential when the secrets seed has it', async () => {
    renderApp(path(seedIntegrationIds.github));

    const slots = await screen.findByRole('list', { name: 'Credential slots' });
    await waitFor(() =>
      expect(within(slots).getByRole('link')).toHaveAttribute(
        'href',
        expect.stringContaining(`/secrets/credentials/${seedIds.credentials.github}`),
      ),
    );
  });

  it('keeps a reference without a link when no credential matches', async () => {
    renderApp(path(seedIntegrationIds.telegram));

    const slots = await screen.findByRole('list', { name: 'Credential slots' });
    expect(within(slots).queryByRole('link')).not.toBeInTheDocument();
  });

  it('warns about a webhook registered with a superseded secret', async () => {
    renderApp(path(seedIntegrationIds.telegram));

    expect(await screen.findByText(/Registered with a superseded secret/)).toBeInTheDocument();
    expect(screen.getAllByText(/superseded secret/).length).toBeGreaterThan(0);
  });

  it('explains the fail-closed state and disables the changes it blocks', async () => {
    renderApp(path(seedIntegrationIds.sentry));

    expect(await screen.findByRole('heading', { name: 'Sentry production' })).toBeInTheDocument();
    expect(screen.getAllByText('Customer managed key').length).toBeGreaterThan(0);
    expect(screen.getByText('Disabled')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Rotate' })).toBeDisabled();
  });

  it('pauses and resumes', async () => {
    const user = userEvent.setup();
    renderApp(path(seedIntegrationIds.github));

    await user.click(await screen.findByRole('button', { name: 'Pause' }));
    const dialog = await screen.findByRole('alertdialog');
    await user.click(within(dialog).getByRole('button', { name: 'Pause' }));

    await user.click(await screen.findByRole('button', { name: 'Resume' }));
    await waitFor(() => expect(screen.getByRole('button', { name: 'Pause' })).toBeInTheDocument());
  });

  it('rotates a slot and then settles at the next version', async () => {
    const user = userEvent.setup();
    renderApp(path(seedIntegrationIds.github));

    await user.click(await screen.findByRole('button', { name: 'Rotate' }));
    const dialog = await screen.findByRole('dialog');
    await user.click(within(dialog).getByRole('button', { name: 'Rotate' }));

    const slots = await screen.findByRole('list', { name: 'Credential slots' });
    await waitFor(() => expect(within(slots).getByText('v3')).toBeInTheDocument());
    expect(within(slots).getByText('Active')).toBeInTheDocument();
  });

  it('re-registers a superseded webhook', async () => {
    const user = userEvent.setup();
    renderApp(path(seedIntegrationIds.telegram));

    await user.click(await screen.findByRole('button', { name: 'Re-register webhook' }));
    const dialog = await screen.findByRole('alertdialog');
    await user.click(within(dialog).getByRole('button', { name: 'Re-register' }));

    await waitFor(() => expect(screen.queryByText(/Registered with a superseded secret/)).not.toBeInTheDocument());
  });

  it('shows not found for an unknown integration', async () => {
    renderApp(path('github/missing'));

    expect(await screen.findByText('Source not found')).toBeInTheDocument();
  });
});
