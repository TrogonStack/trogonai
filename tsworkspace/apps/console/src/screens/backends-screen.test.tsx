import { screen, within } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';

import { createFakeBackend } from '@/fake-backend';
import { keyBackendId } from '@/features/custody/domain/identifiers';
import { renderApp } from '@/test/render-app';

describe('key backends', () => {
  it('lists only the tenant registrations with the three dimensions', async () => {
    renderApp('/keys/backends');

    const table = await screen.findByRole('table');
    expect(await within(table).findByText('Acme AWS production')).toBeInTheDocument();
    expect(within(table).getByText('Acme Google Cloud KMS')).toBeInTheDocument();
    expect(within(table).getByText('Acme datacenter OpenBao')).toBeInTheDocument();
    expect(within(table).queryByText(/Platform key service/)).not.toBeInTheDocument();
    expect(within(table).queryByText(/kbe_platform/)).not.toBeInTheDocument();
    expect(within(table).getAllByText('Enabled').length).toBeGreaterThan(0);
    expect(within(table).getAllByText('Connected').length).toBeGreaterThan(0);
    expect(within(table).getAllByText('Failed').length).toBeGreaterThan(0);
    expect(screen.getByRole('link', { name: 'Register a backend' })).toHaveAttribute('href', '/keys/backends/new');
  });

  it('offers Encryption keys under Security and never claims hardware custody', async () => {
    renderApp('/keys');

    const nav = within(await screen.findByRole('navigation', { name: 'Primary' }));
    expect(nav.getByText('Security')).toBeInTheDocument();
    expect(nav.getByRole('link', { name: /^Encryption keys/ })).toHaveAttribute('href', '/keys');
    expect(await screen.findByRole('heading', { name: 'Encryption keys' })).toBeInTheDocument();
    expect(screen.queryByText(/HSM|FIPS|XKS|EKM/)).not.toBeInTheDocument();
    expect(screen.queryByText(/Customer-operated OpenBao/)).not.toBeInTheDocument();
  });

  it('flags a paused backend in the overview header', async () => {
    const backend = createFakeBackend({ latencyMs: 0 });
    await backend.keyCustody.pauseBackend(keyBackendId('kbe_aws_acme_prod'));
    renderApp('/keys', backend);

    expect(await screen.findByText('1 paused')).toBeInTheDocument();
  });
});

it('offers registration when there are no tenant backends', async () => {
  const backend = createFakeBackend({ latencyMs: 0 });
  const state = await backend.keyCustody.load();
  vi.spyOn(backend.keyCustody, 'load').mockResolvedValue({ ...state, backends: [] });
  renderApp('/keys/backends', backend);
  expect(await screen.findByText('No backends registered')).toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Clear filters' })).not.toBeInTheDocument();
});
