import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';

import { createFakeBackend } from '@/fake-backend';
import { renderApp } from '@/test/render-app';

const PEM = '-----BEGIN CERTIFICATE-----\nMIIBszCCAVmgAwIBAgIUQ0\n-----END CERTIFICATE-----';

describe('register backend wizard', () => {
  it('registers an AWS KMS backend after every check passes', async () => {
    const user = userEvent.setup();
    renderApp('/keys/backends/new');

    await user.click(await screen.findByRole('radio', { name: /AWS KMS/ }));
    await user.click(screen.getByRole('button', { name: 'Continue' }));

    await user.type(screen.getByLabelText('Display name'), 'Acme AWS staging');
    await user.type(screen.getByLabelText('Role ARN'), 'arn:aws:iam::381204965517:role/trogon-key-access');
    await user.click(screen.getByRole('button', { name: 'Generate setup instructions' }));

    expect(await screen.findByText('Your external ID')).toBeInTheDocument();
    expect(screen.getAllByText(/trogon-ext-/).length).toBeGreaterThan(0);
    await user.click(screen.getByRole('button', { name: 'I have applied these' }));

    await user.click(await screen.findByRole('button', { name: 'Run checks' }));
    const checks = await screen.findByRole('list', { name: 'Validation checks' });
    await waitFor(() => expect(within(checks).getAllByLabelText('Passed')).toHaveLength(4));
    expect(within(checks).getByText('AssumeRole without an external ID is denied')).toBeInTheDocument();

    await user.click(screen.getByRole('button', { name: 'Back to instructions' }));
    await user.click(screen.getByRole('button', { name: 'I have applied these' }));
    expect(within(screen.getByRole('list', { name: 'Validation checks' })).getAllByLabelText('Passed')).toHaveLength(4);
    expect(screen.queryByRole('button', { name: 'Run checks' })).not.toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Register backend' }));

    expect(await screen.findByRole('heading', { name: 'Backend registered' })).toBeInTheDocument();
    expect(screen.getByRole('link', { name: 'View backend' })).toBeInTheDocument();
  });

  it('stops at the failing check and explains it', async () => {
    const user = userEvent.setup();
    renderApp('/keys/backends/new');

    await user.click(await screen.findByRole('radio', { name: /AWS KMS/ }));
    await user.click(screen.getByRole('button', { name: 'Continue' }));
    await user.type(screen.getByLabelText('Display name'), 'Acme AWS lax');
    await user.type(screen.getByLabelText('Role ARN'), 'arn:aws:iam::381204965517:role/trogon-no-external-id');
    await user.click(screen.getByRole('button', { name: 'Generate setup instructions' }));
    await user.click(await screen.findByRole('button', { name: 'I have applied these' }));
    await user.click(await screen.findByRole('button', { name: 'Run checks' }));

    await waitFor(() => expect(screen.getByRole('button', { name: 'Run checks again' })).toBeEnabled());
    const checks = screen.getByRole('list', { name: 'Validation checks' });
    expect(within(checks).getByLabelText('Failed')).toBeInTheDocument();
    expect(within(checks).getAllByLabelText('Not run').length).toBeGreaterThan(0);
    expect(within(checks).getByRole('alert')).toBeInTheDocument();
    expect(screen.queryByRole('heading', { name: 'Backend registered' })).not.toBeInTheDocument();
  });

  it('clears a check error when preparing a new registration attempt', async () => {
    const user = userEvent.setup();
    const backend = createFakeBackend({ latencyMs: 0 });
    const runCheck = backend.keyCustody.runCheck.bind(backend.keyCustody);
    let failCheck = true;
    backend.keyCustody.runCheck = async (...args) => {
      if (failCheck) throw new Error('Check transport failed.');
      return runCheck(...args);
    };
    renderApp('/keys/backends/new', backend);

    await user.click(await screen.findByRole('radio', { name: /AWS KMS/ }));
    await user.click(screen.getByRole('button', { name: 'Continue' }));
    await user.type(screen.getByLabelText('Display name'), 'Acme AWS first');
    await user.type(screen.getByLabelText('Role ARN'), 'arn:aws:iam::381204965517:role/trogon-key-access');
    await user.click(screen.getByRole('button', { name: 'Generate setup instructions' }));
    await user.click(await screen.findByRole('button', { name: 'I have applied these' }));
    await user.click(await screen.findByRole('button', { name: 'Run checks' }));
    expect(await screen.findByText('A check could not run')).toBeInTheDocument();

    failCheck = false;
    await user.click(screen.getByRole('button', { name: 'Back to instructions' }));
    await user.click(screen.getByRole('button', { name: 'Back' }));
    await user.clear(screen.getByLabelText('Display name'));
    await user.type(screen.getByLabelText('Display name'), 'Acme AWS second');
    await user.click(screen.getByRole('button', { name: 'Generate setup instructions' }));
    await user.click(await screen.findByRole('button', { name: 'I have applied these' }));

    expect(screen.queryByText('A check could not run')).not.toBeInTheDocument();
  });

  it('asks for a trust root when registering OpenBao and uses the plain product name', async () => {
    const user = userEvent.setup();
    renderApp('/keys/backends/new');

    const openbao = await screen.findByRole('radio', { name: /OpenBao/ });
    expect(openbao).not.toHaveAccessibleName(/Customer-operated/);
    await user.click(openbao);
    await user.click(screen.getByRole('button', { name: 'Continue' }));

    await user.type(screen.getByLabelText('Display name'), 'Acme lab OpenBao');
    await user.type(screen.getByLabelText('Endpoint'), 'https://bao.lab.example:8200');
    await user.type(screen.getByLabelText('Auth role'), 'trogon-transit');
    await user.type(screen.getByLabelText('OpenBao version'), '2.6.1');
    await user.click(screen.getByRole('button', { name: 'Generate setup instructions' }));
    expect(
      await screen.findByText('Paste the PEM encoded trust root certificate the endpoint chains to.'),
    ).toBeInTheDocument();

    await user.click(screen.getByLabelText('Trust root certificate'));
    await user.paste(PEM);
    await user.click(screen.getByRole('button', { name: 'Generate setup instructions' }));
    expect(await screen.findByRole('button', { name: 'I have applied these' })).toBeInTheDocument();
  });

  it('scopes the OpenBao setup commands to the namespace the owner enters', async () => {
    const user = userEvent.setup();
    renderApp('/keys/backends/new');

    await user.click(await screen.findByRole('radio', { name: /OpenBao/ }));
    await user.click(screen.getByRole('button', { name: 'Continue' }));

    await user.type(screen.getByLabelText('Display name'), 'Acme team OpenBao');
    await user.type(screen.getByLabelText('Endpoint'), 'https://bao.lab.example:8200');
    await user.type(screen.getByLabelText('Namespace'), 'team-a');
    await user.type(screen.getByLabelText('Auth role'), 'trogon-transit');
    await user.type(screen.getByLabelText('OpenBao version'), '2.6.1');
    await user.click(screen.getByLabelText('Trust root certificate'));
    await user.paste(PEM);
    await user.click(screen.getByRole('button', { name: 'Generate setup instructions' }));

    expect(await screen.findByText(/export BAO_NAMESPACE=team-a/)).toBeInTheDocument();
  });
});
