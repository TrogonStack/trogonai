import { screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';

import { renderApp } from '@/test/render-app';

describe('registration form', () => {
  it('shows field-level errors mirroring the contract invariants and does not submit', async () => {
    const user = userEvent.setup();
    const { client } = renderApp('/providers/new');

    await user.click(await screen.findByRole('button', { name: 'Register connection' }));

    expect(await screen.findByText('Connection id is required.')).toBeInTheDocument();
    expect(await screen.findByText(/fields? needs? attention/)).toBeInTheDocument();
    expect(screen.getByText('Parent node id is required.')).toBeInTheDocument();
    expect(screen.getByText('Provider id is required.')).toBeInTheDocument();
    expect(screen.getByText(/credential-free https URL/)).toBeInTheDocument();
    expect(screen.getByText('Model identifier is required.')).toBeInTheDocument();
    expect(screen.getByText(/Managed offerings require an exact catalog pin/)).toBeInTheDocument();
    expect(screen.getByText('Select at least one protocol.')).toBeInTheDocument();
    expect(screen.getByText('Binding id is required.')).toBeInTheDocument();
    expect(screen.getByText('Secret reference is required.')).toBeInTheDocument();
    expect((await client.list()).some((record) => record.connection.id?.value === '')).toBe(false);
  });

  it('shows the output limit field only when an OpenAI protocol is selected', async () => {
    const user = userEvent.setup();
    renderApp('/providers/new');
    await screen.findByRole('form', { name: 'Register provider connection' });

    expect(screen.queryByLabelText('OpenAI output limit field')).not.toBeInTheDocument();
    await user.click(screen.getByRole('checkbox', { name: 'OpenAI Chat Completions' }));
    expect(screen.getByLabelText('OpenAI output limit field')).toBeInTheDocument();
    await user.click(screen.getByRole('checkbox', { name: 'OpenAI Chat Completions' }));
    expect(screen.queryByLabelText('OpenAI output limit field')).not.toBeInTheDocument();
  });

  it('rejects an http base URL and a pasted credential reference', async () => {
    const user = userEvent.setup();
    renderApp('/providers/new');
    await screen.findByRole('form', { name: 'Register provider connection' });

    await user.type(screen.getByLabelText('Base URL'), 'http://api.example.com');
    await user.type(screen.getByLabelText('Secret reference'), 'sk-live-not-a-reference');
    await user.click(screen.getByRole('button', { name: 'Register connection' }));

    expect(await screen.findByText(/credential-free https URL/)).toBeInTheDocument();
    expect(screen.getByText(/Never paste a credential/)).toBeInTheDocument();
  });

  it('registers a connection from a preset and opens its detail page', async () => {
    const user = userEvent.setup();
    const { client } = renderApp('/providers/new');
    await screen.findByRole('form', { name: 'Register provider connection' });

    await user.click(screen.getByRole('button', { name: 'Browse presets' }));
    await user.click(await screen.findByRole('button', { name: /^Cloudflare AI Gateway/ }));
    await user.type(screen.getByLabelText('Connection id'), 'cf-gateway-test');
    await user.type(screen.getByLabelText('Parent node id'), 'platform-team');
    await user.type(screen.getByLabelText('Binding id'), 'cf-gateway-test-key-1');
    await user.type(screen.getByLabelText('Display name'), 'Cloudflare test');
    await user.click(screen.getByRole('button', { name: 'Register connection' }));

    expect(await screen.findByRole('heading', { name: 'Cloudflare test' })).toBeInTheDocument();
    expect(screen.getAllByText('Pending').length).toBeGreaterThan(0);
    const stored = await client.get('cf-gateway-test');
    expect(stored?.revision).toBe(1);
  });

  it('shows a decider fault when the connection id already exists', async () => {
    const user = userEvent.setup();
    renderApp('/providers/new');
    await screen.findByRole('form', { name: 'Register provider connection' });

    await user.click(screen.getByRole('button', { name: 'Browse presets' }));
    await user.click(await screen.findByRole('button', { name: /^Cloudflare AI Gateway/ }));
    await user.type(screen.getByLabelText('Connection id'), 'openai-prod');
    await user.type(screen.getByLabelText('Parent node id'), 'platform-team');
    await user.type(screen.getByLabelText('Binding id'), 'fresh-binding');
    await user.click(screen.getByRole('button', { name: 'Register connection' }));

    expect(await screen.findByText('PROVIDER_CONNECTION_ALREADY_EXISTS')).toBeInTheDocument();
  });
});
