import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';

import { createFakeBackend } from '@/fake-backend';
import { keyBackendId, keyRef, purposeId } from '@/features/custody/domain/identifiers';
import { seedKeyRefs, seedLocators } from '@/features/custody/fake/seed-ids';
import { renderApp } from '@/test/render-app';

describe('key detail', () => {
  it('lists every provider attempt with the identifier the provider returned or why there is none', async () => {
    renderApp(`/keys/keys/${seedKeyRefs.awsMessages}`);

    expect(await screen.findByRole('heading', { name: seedKeyRefs.awsMessages })).toBeInTheDocument();
    const records = screen.getByText('Usage records').closest('[data-slot="card"]') as HTMLElement;
    expect(within(records).getByText('7c3e91a2-5b84-4f0d-a6e1-2d9c8b40f517')).toBeInTheDocument();
    expect(within(records).getByText('3d8a6f10-27c4-4be9-b5e1-9a0c72d4f385')).toBeInTheDocument();
    expect(within(records).getByText('b94c1e07-6a52-4d38-87f3-1e5d09a2c6b4')).toBeInTheDocument();
    expect(within(records).getByText('Not provided by provider')).toBeInTheDocument();
    expect(within(records).getAllByText('Attempt 1').length).toBeGreaterThan(0);
    expect(within(records).getByText('Attempt 2')).toBeInTheDocument();
    expect(within(records).getByText(/The identifier the provider returned for this attempt\./)).toBeInTheDocument();
    expect(within(records).queryByText(/CloudTrail|audit log/i)).not.toBeInTheDocument();
  });

  it('shows No response when the provider never answered', async () => {
    renderApp(`/keys/keys/${seedKeyRefs.openbaoArtifacts}`);

    const records = (await screen.findByText('Usage records')).closest('[data-slot="card"]') as HTMLElement;
    expect(within(records).getAllByText('No response').length).toBeGreaterThan(0);
  });

  it('reads the locator back only in the owner customer managed binding', async () => {
    renderApp(`/keys/keys/${seedKeyRefs.awsMessages}`);

    expect(await screen.findByText(seedLocators.awsMessages)).toBeInTheDocument();
    expect(screen.getByText(/Visible only to you/)).toBeInTheDocument();
  });

  it('shows a dedicated key as managed by the platform with no locator, provider or usage records', async () => {
    renderApp(`/keys/keys/${seedKeyRefs.dedicatedArtifacts}`);

    expect(await screen.findByRole('heading', { name: seedKeyRefs.dedicatedArtifacts })).toBeInTheDocument();
    expect(screen.getAllByText('Managed by the platform').length).toBeGreaterThan(0);
    expect(screen.getByText('Dedicated key')).toBeInTheDocument();
    expect(screen.queryByText('Provider locator')).not.toBeInTheDocument();
    expect(screen.queryByText('Usage records')).not.toBeInTheDocument();
    expect(screen.queryByText(/Transit|transit\/|platform-request-never-shown|kbe_platform/)).not.toBeInTheDocument();
    expect(screen.queryByText(/HSM|FIPS|XKS|EKM/)).not.toBeInTheDocument();
  });

  it('makes a key the default and refuses it for a decrypt only key', async () => {
    const user = userEvent.setup();
    renderApp(`/keys/keys/${seedKeyRefs.openbaoArtifacts}`);

    const toggle = await screen.findByRole('switch', { name: /Default key for/ });
    expect(toggle).toHaveAttribute('aria-checked', 'false');
    await user.click(toggle);
    await waitFor(() =>
      expect(screen.getByRole('switch', { name: /Default key for/ })).toHaveAttribute('aria-checked', 'true'),
    );
  });

  it('disables set as default for a key that is not active', async () => {
    renderApp(`/keys/keys/${seedKeyRefs.awsLegacyMessages}`);

    const toggle = await screen.findByRole('switch', { name: /Default key for/ });
    expect(toggle).toBeDisabled();
    expect(screen.getAllByText('Decrypt only').length).toBeGreaterThan(0);
  });

  it('disables set as default while the purpose is being migrated', async () => {
    const backend = createFakeBackend({ latencyMs: 0 });
    await backend.keyCustody.startMigration({ purpose: purposeId('session_artifacts'), target: { tier: 'managed' } });
    renderApp(`/keys/keys/${seedKeyRefs.openbaoArtifacts}`, backend);

    const toggle = await screen.findByRole('switch', { name: /Default key for/ });
    expect(toggle).toBeDisabled();
    expect(screen.getByText('A migration is already moving payloads for this purpose.')).toBeInTheDocument();
  });

  it('shows the fail closed notice for a disabled key', async () => {
    renderApp(`/keys/keys/${seedKeyRefs.gcpInteractions}`);

    expect(await screen.findByText(/The platform fails closed/)).toBeInTheDocument();
    expect(screen.getAllByText('Disabled').length).toBeGreaterThan(0);
  });

  it('denies a paused backend on the key and blocks setting the default', async () => {
    const backend = createFakeBackend({ latencyMs: 0 });
    await backend.keyCustody.pauseBackend(keyBackendId('kbe_obao_acme_dc'));
    renderApp(`/keys/keys/${seedKeyRefs.openbaoArtifacts}`, backend);

    expect((await screen.findAllByText(/The backend is paused\. Resume it/)).length).toBeGreaterThan(0);
    expect(screen.getByRole('switch', { name: /Default key for/ })).toBeDisabled();
  });

  it('reports a missing key', async () => {
    renderApp(`/keys/keys/${keyRef('kr_00000000')}`);
    expect(await screen.findByText('Key not found')).toBeInTheDocument();
  });
});
