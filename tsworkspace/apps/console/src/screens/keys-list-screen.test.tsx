import { screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { KeyState } from '@trogonai/proto';
import { describe, expect, it } from 'vitest';

import { createFakeBackend } from '@/fake-backend';
import { seedKeyRefs } from '@/features/custody/fake/seed-ids';
import { renderApp } from '@/test/render-app';

describe('keys list', () => {
  it('lists dedicated and customer managed keys with their state and no platform backend identity', async () => {
    renderApp('/keys/keys');

    const table = await screen.findByRole('table');
    expect(await within(table).findByText(seedKeyRefs.awsMessages)).toBeInTheDocument();
    expect(within(table).getByText(seedKeyRefs.dedicatedArtifacts)).toBeInTheDocument();
    expect(within(table).getAllByText('Default')).toHaveLength(3);
    expect(within(table).getAllByText('Managed by the platform').length).toBeGreaterThan(0);
    expect(within(table).getByText('Decrypt only')).toBeInTheDocument();
    expect(within(table).getByText('Disabled')).toBeInTheDocument();
    expect(within(table).queryByText(/kbe_platform|Platform key service/)).not.toBeInTheDocument();
    expect(screen.getByRole('link', { name: 'Bind a key' })).toHaveAttribute('href', '/keys/keys/new');
  });

  it('links the Keys subnav tab and opens a key from the list', async () => {
    const user = userEvent.setup();
    renderApp('/keys/keys');

    const subnav = await screen.findByRole('navigation', { name: 'Encryption keys' });
    expect(within(subnav).getByRole('link', { name: 'Keys' })).toHaveAttribute('href', '/keys/keys');
    await user.click(await screen.findByRole('link', { name: seedKeyRefs.openbaoArtifacts }));
    expect(await screen.findByRole('heading', { name: seedKeyRefs.openbaoArtifacts })).toBeInTheDocument();
  });

  it('shows per-purpose custody on the overview with the tier and the owning service', async () => {
    renderApp('/keys');

    const panel = (await screen.findByText('Current custody')).closest('[data-slot="card"]') as HTMLElement;
    const messages = within(panel).getByRole('listitem', { name: 'Session messages' });
    expect(within(messages).getByText('Customer managed key')).toBeInTheDocument();
    expect(within(messages).getByText('Acme AWS production')).toBeInTheDocument();
    expect(within(messages).getByText('Session service')).toBeInTheDocument();
    const artifacts = within(panel).getByRole('listitem', { name: 'Session artifacts' });
    expect(within(artifacts).getByText('Dedicated key')).toBeInTheDocument();
    expect(within(artifacts).getAllByText('Managed by the platform').length).toBeGreaterThan(0);
    const interactions = within(panel).getByRole('listitem', { name: /Workflow run interactions/ });
    expect(within(interactions).getByText('Workflow service')).toBeInTheDocument();
    expect(within(panel).queryByText(/credential|secret/i)).not.toBeInTheDocument();
  });

  it('links bound keys from the backend and offers Bind a key with the backend preselected', async () => {
    renderApp('/keys/backends/kbe_aws_acme_prod');

    expect(await screen.findByRole('link', { name: seedKeyRefs.awsMessages })).toHaveAttribute(
      'href',
      `/keys/keys/${seedKeyRefs.awsMessages}`,
    );
    expect(screen.getByRole('link', { name: 'Bind a key' })).toHaveAttribute(
      'href',
      '/keys/keys/new?backend=kbe_aws_acme_prod',
    );
  });

  it('labels all five key states', async () => {
    const backend = createFakeBackend({ latencyMs: 0 });
    await backend.keyCustody.simulateKeyState(seedKeyRefs.awsLegacyMessages, KeyState.PENDING_DESTRUCTION);
    await backend.keyCustody.simulateKeyState(seedKeyRefs.gcpInteractions, KeyState.DESTROYED);
    renderApp('/keys/keys', backend);

    const table = await screen.findByRole('table');
    for (const label of ['Active', 'Pending destruction', 'Destroyed']) {
      expect((await within(table).findAllByText(label)).length).toBeGreaterThan(0);
    }
  });
});
