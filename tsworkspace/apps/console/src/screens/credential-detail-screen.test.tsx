import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';

import { seedIds } from '@/features/secrets/fake/seed';
import { renderApp } from '@/test/render-app';

const path = (secret: string) => `/secrets/credentials/${secret}`;

async function openActions(user: ReturnType<typeof userEvent.setup>) {
  await user.click(await screen.findByRole('button', { name: 'Credential actions' }));
}

describe('credential detail', () => {
  it('shows the stable reference, immutable policy and version ladder', async () => {
    renderApp(path(seedIds.credentials.openai));

    expect(await screen.findByRole('heading', { name: 'OpenAI production key' })).toBeInTheDocument();
    const policy = screen.getByLabelText('Immutable policy');
    expect(within(policy).getByText('Provider authentication')).toBeInTheDocument();
    expect(within(policy).getByText('API token')).toBeInTheDocument();
    expect(within(policy).getByText('Production')).toBeInTheDocument();
    expect(screen.getByText(seedIds.credentials.openai)).toBeInTheDocument();
    expect(screen.getByRole('list', { name: 'Version ladder' })).toBeInTheDocument();
  });

  it('rotates to a new version and keeps the secret reference', async () => {
    const user = userEvent.setup();
    const { router } = renderApp(path(seedIds.credentials.openai));

    const ladderBefore = await screen.findByRole('list', { name: 'Version ladder' });
    const before = within(ladderBefore).getAllByRole('listitem').length;

    await user.click(await screen.findByRole('button', { name: 'Rotate' }));
    const dialog = await screen.findByRole('dialog');
    await user.type(within(dialog).getByLabelText('Secret material'), 'seed-openai-token-v4');
    await user.click(within(dialog).getByRole('button', { name: 'Rotate' }));

    await waitFor(() => {
      const ladder = screen.getByRole('list', { name: 'Version ladder' });
      expect(within(ladder).getAllByRole('listitem').length).toBeGreaterThan(before);
    });
    expect(router.state.location.pathname).toBe(path(seedIds.credentials.openai));
    expect(screen.getByText(seedIds.credentials.openai)).toBeInTheDocument();
    expect(document.body.textContent).not.toContain('seed-openai-token-v4');
  });

  it('makes revocation irreversible', async () => {
    const user = userEvent.setup();
    renderApp(path(seedIds.credentials.openai));

    await openActions(user);
    await user.click(await screen.findByRole('menuitem', { name: 'Revoke' }));
    const dialog = await screen.findByRole('alertdialog');
    await user.click(within(dialog).getByRole('button', { name: 'Revoke credential' }));

    await waitFor(() => expect(screen.getAllByText('Revoked').length).toBeGreaterThan(0));
    await waitFor(() => expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument());
    expect(screen.queryByRole('button', { name: 'Rotate' })).not.toBeInTheDocument();
    await openActions(user);
    expect(screen.queryByRole('menuitem', { name: 'Enable' })).not.toBeInTheDocument();
    expect(screen.queryByRole('menuitem', { name: 'Disable' })).not.toBeInTheDocument();
  });

  it('shows a not found state for an unknown reference', async () => {
    renderApp(path('does-not-exist'));

    expect(await screen.findByRole('heading', { name: 'Credential not found' })).toBeInTheDocument();
  });
});
