import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';

import { createFakeBackend } from '@/fake-backend';
import { keyBackendId } from '@/features/custody/domain/identifiers';
import { renderApp } from '@/test/render-app';

const NEW_ARN = 'arn:aws:kms:eu-west-1:512049177301:key/1c0e7d52-9b4a-4f6e-8a31-6d2f90b7c418';

describe('bind key wizard', () => {
  it('binds a key against a purpose from the catalogue after every check passes', async () => {
    const user = userEvent.setup();
    renderApp('/keys/keys/new?backend=kbe_aws_acme_prod');

    expect(await screen.findByRole('radio', { name: /Acme AWS production/ })).toBeChecked();
    await user.click(screen.getByRole('button', { name: 'Continue' }));

    await user.type(screen.getByLabelText(/key locator/), NEW_ARN);
    expect(screen.getByRole('radio', { name: /Session messages/ })).toBeInTheDocument();
    expect(screen.getByRole('radio', { name: /Session artifacts/ })).toBeInTheDocument();
    expect(screen.getByRole('radio', { name: /Workflow run interactions/ })).toBeInTheDocument();
    expect(screen.queryByRole('radio', { name: /credential|secret/i })).not.toBeInTheDocument();
    await user.click(screen.getByRole('radio', { name: /Session artifacts/ }));
    await user.click(screen.getByRole('button', { name: 'Continue to validation' }));

    await user.click(await screen.findByRole('button', { name: 'Run checks' }));
    const checks = await screen.findByRole('list', { name: 'Validation checks' });
    await waitFor(() => expect(within(checks).getAllByLabelText('Passed')).toHaveLength(3));
    await user.click(screen.getByRole('button', { name: 'Bind key' }));

    expect(await screen.findByRole('heading', { name: 'Key bound' })).toBeInTheDocument();
    expect(screen.getByRole('link', { name: 'View key' })).toBeInTheDocument();
  });

  it('keeps the bound key visible when the custody refresh fails', async () => {
    const user = userEvent.setup();
    const backend = createFakeBackend({ latencyMs: 0 });
    const load = backend.keyCustody.load.bind(backend.keyCustody);
    let failRefresh = false;
    backend.keyCustody.load = async () => {
      if (failRefresh) throw new Error('Custody refresh failed.');
      return load();
    };
    renderApp('/keys/keys/new?backend=kbe_aws_acme_prod', backend);

    await user.click(await screen.findByRole('button', { name: 'Continue' }));
    await user.type(screen.getByLabelText(/key locator/), NEW_ARN);
    await user.click(screen.getByRole('radio', { name: /Session artifacts/ }));
    await user.click(screen.getByRole('button', { name: 'Continue to validation' }));
    await user.click(await screen.findByRole('button', { name: 'Run checks' }));
    const checks = await screen.findByRole('list', { name: 'Validation checks' });
    await waitFor(() => expect(within(checks).getAllByLabelText('Passed')).toHaveLength(3));

    failRefresh = true;
    await user.click(screen.getByRole('button', { name: 'Bind key' }));

    expect(await screen.findByText('Could not refresh backends')).toBeInTheDocument();
    expect(screen.queryByRole('link', { name: 'View key' })).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'View key unavailable' })).toBeDisabled();
  });

  it('rejects an alias before anything is prepared', async () => {
    const user = userEvent.setup();
    renderApp('/keys/keys/new?backend=kbe_aws_acme_prod');

    await user.click(await screen.findByRole('button', { name: 'Continue' }));
    await user.type(screen.getByLabelText(/key locator/), 'arn:aws:kms:eu-west-1:512049177301:alias/prod');
    await user.click(screen.getByRole('button', { name: 'Continue to validation' }));

    expect(await screen.findByText(/This is an alias/)).toBeInTheDocument();
  });

  it('explains when no key purpose is available and prevents validation', async () => {
    const user = userEvent.setup();
    const backend = createFakeBackend({ latencyMs: 0 });
    const load = backend.keyCustody.load.bind(backend.keyCustody);
    backend.keyCustody.load = async () => ({ ...(await load()), purposes: [] });
    renderApp('/keys/keys/new?backend=kbe_aws_acme_prod', backend);

    await user.click(await screen.findByRole('button', { name: 'Continue' }));

    expect(screen.getByText('No key purposes are available.')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Continue to validation' })).toBeDisabled();
  });

  it('denies a paused backend with BackendPaused and does not prepare the key', async () => {
    const user = userEvent.setup();
    const backend = createFakeBackend({ latencyMs: 0 });
    await backend.keyCustody.pauseBackend(keyBackendId('kbe_aws_acme_prod'));
    renderApp('/keys/keys/new?backend=kbe_aws_acme_prod', backend);

    await user.click(await screen.findByRole('button', { name: 'Continue' }));
    await user.type(screen.getByLabelText(/key locator/), NEW_ARN);
    await user.click(screen.getByRole('button', { name: 'Continue to validation' }));

    const alert = (await screen.findByText('The key could not be prepared')).closest('[role="alert"]');
    expect(alert).toHaveTextContent('KEY_USE_DENIED');
    expect(alert).toHaveTextContent('The backend is paused');
    expect(screen.queryByRole('button', { name: 'Run checks' })).not.toBeInTheDocument();
  });
});
