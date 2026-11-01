import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { KeyBackendFailureReason } from '@trogonai/proto';
import { describe, expect, it } from 'vitest';

import { createFakeBackend } from '@/fake-backend';
import { keyBackendId } from '@/features/custody/domain/identifiers';
import { renderApp } from '@/test/render-app';

const PAUSE_COPY =
  'Pausing stops the platform from calling your provider. To revoke access, disable the key or revoke access in your provider.';

describe('backend detail', () => {
  it('shows a connected backend with absent expiry signals left absent', async () => {
    renderApp('/keys/backends/kbe_aws_acme_prod');

    expect(await screen.findByRole('heading', { name: 'Acme AWS production' })).toBeInTheDocument();
    const connection = screen.getByText('Connection status').closest('[data-slot="card"]') as HTMLElement;
    expect(within(connection).getByText('Connected')).toBeInTheDocument();
    expect(screen.getAllByText('Not reported').length).toBeGreaterThan(0);
    expect(screen.getByText('512049177301')).toBeInTheDocument();
  });

  it('keeps cached backend details visible when a background refresh fails', async () => {
    const backend = createFakeBackend({ latencyMs: 0 });
    const load = backend.keyCustody.load.bind(backend.keyCustody);
    let failRefresh = false;
    backend.keyCustody.load = async () => {
      if (failRefresh) throw new Error('Custody refresh failed.');
      return load();
    };
    const { queryClient } = renderApp('/keys/backends/kbe_aws_acme_prod', backend);

    expect(await screen.findByRole('heading', { name: 'Acme AWS production' })).toBeInTheDocument();

    failRefresh = true;
    await queryClient.invalidateQueries({ queryKey: ['key-custody'] });

    const alert = await screen.findByRole('alert');
    expect(screen.getByRole('heading', { name: 'Acme AWS production' })).toBeInTheDocument();
    expect(alert).toHaveTextContent('Could not refresh the backend');
    expect(alert).toHaveTextContent('Custody refresh failed.');
  });

  it('shows the typed failure reason and the expiry warning', async () => {
    renderApp('/keys/backends/kbe_obao_acme_dc');

    expect(await screen.findByRole('heading', { name: 'Acme datacenter OpenBao' })).toBeInTheDocument();
    const connection = screen.getByText('Connection status').closest('[data-slot="card"]') as HTMLElement;
    expect(within(connection).getByText('Failed')).toBeInTheDocument();
    expect(within(connection).getByText('Timed out')).toBeInTheDocument();
    const warnings = screen.getByRole('list', { name: 'Expiry warnings' });
    expect(within(warnings).getByText(/Endpoint server certificate expires in 1[12] days/)).toBeInTheDocument();
    expect(within(warnings).queryByText(/Client certificate/)).not.toBeInTheDocument();
  });

  it('pauses with the exact copy, blocks probes while paused, and resumes after validating', async () => {
    const user = userEvent.setup();
    renderApp('/keys/backends/kbe_aws_acme_prod');

    await user.click(await screen.findByRole('button', { name: 'Pause registration' }));
    const dialog = await screen.findByRole('alertdialog');
    expect(within(dialog).getByText(PAUSE_COPY)).toBeInTheDocument();
    await user.click(within(dialog).getByRole('button', { name: 'Pause registration' }));

    expect(await screen.findByRole('button', { name: 'Resume and validate' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Check connection' })).toBeDisabled();
    expect(screen.getByText(/denied with\s+backend paused/)).toBeInTheDocument();

    await user.click(screen.getByRole('button', { name: 'Resume and validate' }));
    expect(await screen.findByRole('button', { name: 'Pause registration' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Check connection' })).toBeEnabled();
  });

  it('stays paused with the validation reason when resume cannot validate', async () => {
    const user = userEvent.setup();
    const view = renderApp('/keys/backends/kbe_aws_acme_prod');

    await user.click(await screen.findByRole('button', { name: 'Pause registration' }));
    await user.click(
      within(await screen.findByRole('alertdialog')).getByRole('button', { name: 'Pause registration' }),
    );
    await screen.findByRole('button', { name: 'Resume and validate' });

    await view.custody.simulateProviderOutage(keyBackendId('kbe_aws_acme_prod'), KeyBackendFailureReason.UNREACHABLE);
    await user.click(screen.getByRole('button', { name: 'Resume and validate' }));

    const alert = await screen.findByText('The registration was not resumed');
    expect(alert.closest('[role="alert"]')).toHaveTextContent('RESUME_VALIDATION_FAILED');
    await waitFor(() => expect(screen.getByRole('button', { name: 'Resume and validate' })).toBeEnabled());
  });

  it('reports a missing backend', async () => {
    renderApp('/keys/backends/kbe_missing');
    expect(await screen.findByText('Backend not found')).toBeInTheDocument();
  });
});
