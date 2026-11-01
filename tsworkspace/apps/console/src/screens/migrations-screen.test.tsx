import { act, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { KeyState } from '@trogonai/proto';
import { describe, expect, it } from 'vitest';

import { createFakeBackend } from '@/fake-backend';
import { keyBackendId, purposeId } from '@/features/custody/domain/identifiers';
import { seedKeyRefs } from '@/features/custody/fake/seed-ids';
import { custodyKeys } from '@/features/custody/queries';
import { renderApp } from '@/test/render-app';

const artifacts = purposeId('session_artifacts');

async function openChangeCustody(user: ReturnType<typeof userEvent.setup>, purposeLabel: string) {
  const card = (await screen.findByRole('listitem', { name: purposeLabel })) as HTMLElement;
  await user.click(within(card).getByRole('button', { name: 'Change custody' }));
  return screen.findByRole('dialog');
}

describe('migrations list', () => {
  it('lists every migration with its direction, status and progress', async () => {
    renderApp('/keys/migrations');

    expect(await screen.findByRole('heading', { name: 'Migrations' })).toBeInTheDocument();
    const table = await screen.findByRole('table');
    expect(within(table).getByText('mig_5d2a91c4')).toBeInTheDocument();
    expect(within(table).getByText('mig_a3f07be8')).toBeInTheDocument();
    expect(within(table).getByText('mig_c19e4d70')).toBeInTheDocument();
    expect(within(table).getByText('Into customer managed custody')).toBeInTheDocument();
    expect(within(table).getByText('Out of customer managed custody')).toBeInTheDocument();
    expect(within(table).getByRole('progressbar', { name: 'Migration mig_5d2a91c4 progress' })).toBeInTheDocument();
    expect(screen.getByText(/not a response to compromise/)).toBeInTheDocument();
  });

  it('links to the migrations list from the keys subnav', async () => {
    renderApp('/keys');

    const subnav = await screen.findByRole('navigation', { name: 'Encryption keys' });
    expect(within(subnav).getByRole('link', { name: 'Migrations' })).toHaveAttribute('href', '/keys/migrations');
  });

  it('starts a migration from the list with a purpose picker', async () => {
    const user = userEvent.setup();
    renderApp('/keys/migrations');

    await user.click(await screen.findByRole('button', { name: 'Start a migration' }));
    const dialog = await screen.findByRole('dialog');
    await user.selectOptions(within(dialog).getByLabelText('Purpose'), 'Session artifacts');
    expect(within(dialog).getByRole('radio', { name: /Managed/ })).toBeInTheDocument();
    expect(within(dialog).queryByRole('radio', { name: /Dedicated/ })).not.toBeInTheDocument();
  });
});

describe('migration detail', () => {
  it('shows progress, counters and both ends of a running migration', async () => {
    renderApp('/keys/migrations/mig_5d2a91c4');

    expect(await screen.findByRole('heading', { name: 'Rewrap session messages' })).toBeInTheDocument();
    expect(screen.getByRole('progressbar', { name: 'Migration mig_5d2a91c4 progress' })).toBeInTheDocument();
    expect(screen.getByText('Rewrapped')).toBeInTheDocument();
    expect(screen.getByText('Remaining')).toBeInTheDocument();
    expect(screen.getByText('Conflicts retried')).toBeInTheDocument();
    expect(screen.getByText(/Still needed to unwrap payloads that have not moved/)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Pause' })).toBeInTheDocument();
  });

  it('pauses and resumes a running migration', async () => {
    const user = userEvent.setup();
    renderApp('/keys/migrations/mig_5d2a91c4');

    await user.click(await screen.findByRole('button', { name: 'Pause' }));
    expect(await screen.findByRole('button', { name: 'Resume' })).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Resume' }));
    expect(await screen.findByRole('button', { name: 'Pause' })).toBeInTheDocument();
  });

  it('shows a completed migration without actions', async () => {
    renderApp('/keys/migrations/mig_a3f07be8');

    expect(await screen.findByRole('heading', { name: 'Rewrap session messages' })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Pause' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Resume' })).not.toBeInTheDocument();
    expect(screen.getByText(/Nothing is wrapped under this key/)).toBeInTheDocument();
  });

  it('says so when the migration does not exist', async () => {
    const backend = createFakeBackend({ latencyMs: 0 });
    const { queryClient } = renderApp('/keys/migrations/mig_missing', backend);

    expect(await screen.findByText('Migration not found')).toBeInTheDocument();

    const load = backend.keyCustody.load.bind(backend.keyCustody);
    let finishRefetch: (() => void) | undefined;
    backend.keyCustody.load = async () => {
      await new Promise<void>((resolve) => {
        finishRefetch = resolve;
      });
      return load();
    };

    void queryClient.invalidateQueries({ queryKey: custodyKeys.state });
    await waitFor(() => expect(queryClient.getQueryState(custodyKeys.state)?.fetchStatus).toBe('fetching'));

    expect(screen.getByText('Migration not found')).toBeInTheDocument();
    expect(screen.queryByText('Loading migration')).not.toBeInTheDocument();

    await act(async () => finishRefetch?.());
  });
});

describe('change custody', () => {
  it('moves a dedicated purpose into customer managed custody and opens the migration', async () => {
    const user = userEvent.setup();
    renderApp('/keys');

    const dialog = await openChangeCustody(user, 'Session artifacts');
    await user.click(within(dialog).getByRole('radio', { name: /Customer managed/ }));
    await user.selectOptions(within(dialog).getByLabelText('Customer managed key'), seedKeyRefs.openbaoArtifacts);
    await user.click(within(dialog).getByRole('button', { name: 'Change custody' }));

    expect(await screen.findByRole('heading', { name: 'Rewrap session artifacts' })).toBeInTheDocument();
    expect(screen.getByText('Into customer managed custody')).toBeInTheDocument();
  });

  it('moves a customer managed purpose back to managed custody', async () => {
    const user = userEvent.setup();
    const backend = createFakeBackend({ latencyMs: 0 });
    await backend.keyCustody.simulateKeyState(seedKeyRefs.gcpInteractions, KeyState.ACTIVE);
    renderApp('/keys', backend);

    const dialog = await openChangeCustody(user, 'Workflow run interactions');
    expect(within(dialog).getByRole('radio', { name: /Dedicated/ })).toBeInTheDocument();
    expect(within(dialog).getByText(/rewraps the 6 existing payloads/)).toBeInTheDocument();
    await user.click(within(dialog).getByRole('radio', { name: /Managed/ }));
    await user.click(within(dialog).getByRole('button', { name: 'Change custody' }));

    expect(await screen.findByRole('heading', { name: 'Rewrap workflow run interactions' })).toBeInTheDocument();
    expect(screen.getByText('Out of customer managed custody')).toBeInTheDocument();
  });

  it('offers a bind link when no customer managed key can take the purpose', async () => {
    const user = userEvent.setup();
    renderApp('/keys');

    const dialog = await openChangeCustody(user, 'Workflow run interactions');
    await user.click(within(dialog).getByRole('radio', { name: /Customer managed/ }));
    expect(within(dialog).getByRole('link', { name: 'Bind a key' })).toHaveAttribute('href', '/keys/keys/new');
    expect(within(dialog).getByRole('button', { name: 'Change custody' })).toBeDisabled();
  });

  it('denies a paused source backend with BackendPaused', async () => {
    const user = userEvent.setup();
    const backend = createFakeBackend({ latencyMs: 0 });
    await backend.keyCustody.simulateKeyState(seedKeyRefs.gcpInteractions, KeyState.ACTIVE);
    await backend.keyCustody.pauseBackend(keyBackendId('kbe_gcp_acme_prod'));
    renderApp('/keys', backend);

    const dialog = await openChangeCustody(user, 'Workflow run interactions');
    await user.click(within(dialog).getByRole('button', { name: 'Change custody' }));

    const alert = (await within(dialog).findByText('The change was rejected')).closest('[role="alert"]');
    expect(alert).toHaveTextContent('KEY_USE_DENIED');
    expect(alert).toHaveTextContent('The backend is paused');
  });

  it('denies a paused target backend with BackendPaused', async () => {
    const user = userEvent.setup();
    const backend = createFakeBackend({ latencyMs: 0 });
    await backend.keyCustody.pauseBackend(keyBackendId('kbe_obao_acme_dc'));
    renderApp('/keys', backend);

    const dialog = await openChangeCustody(user, 'Session artifacts');
    await user.click(within(dialog).getByRole('radio', { name: /Customer managed/ }));
    await user.click(within(dialog).getByRole('button', { name: 'Change custody' }));

    const alert = (await within(dialog).findByText('The change was rejected')).closest('[role="alert"]');
    expect(alert).toHaveTextContent('KEY_USE_DENIED');
    expect(alert).toHaveTextContent('The backend is paused');
  });

  it('surfaces a migration that started while the dialog was open', async () => {
    const user = userEvent.setup();
    let now = new Date('2026-11-01T00:00:00Z');
    const backend = createFakeBackend({ latencyMs: 0, clock: () => now });
    await backend.keyCustody.load();
    now = new Date('2026-11-01T01:00:00Z');
    await backend.keyCustody.load();
    renderApp('/keys', backend);

    const dialog = await openChangeCustody(user, 'Session artifacts');
    await backend.keyCustody.startMigration({ purpose: artifacts, target: { tier: 'managed' } });
    await user.click(within(dialog).getByRole('button', { name: 'Change custody' }));

    const alert = (await within(dialog).findByText('The change was rejected')).closest('[role="alert"]');
    expect(alert).toHaveTextContent('MIGRATION_IN_FLIGHT');
    expect(await within(dialog).findByText(/A migration is already running/)).toBeInTheDocument();
    expect(within(dialog).getByRole('button', { name: 'Change custody' })).toBeDisabled();
  });

  it('disables change custody while a migration runs for that purpose', async () => {
    renderApp('/keys');

    const running = (await screen.findByRole('listitem', { name: 'Session messages' })) as HTMLElement;
    expect(within(running).getByRole('button', { name: 'Change custody' })).toBeDisabled();
    expect(within(running).getByText('Wait for the running migration to finish first.')).toBeInTheDocument();
    expect(within(running).getByRole('link', { name: 'mig_5d2a91c4' })).toBeInTheDocument();
    expect(within(running).getByRole('progressbar')).toBeInTheDocument();

    const idle = screen.getByRole('listitem', { name: 'Session artifacts' });
    expect(within(idle).getByRole('button', { name: 'Change custody' })).toBeEnabled();
  });
});
