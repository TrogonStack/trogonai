import { screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';

import { renderApp } from '@/test/render-app';

describe('secrets overview', () => {
  it('lists the seeded credentials, vaults and summary', async () => {
    renderApp('/secrets');

    expect(await screen.findByRole('heading', { name: 'Secrets' })).toBeInTheDocument();
    expect(await screen.findByRole('link', { name: 'OpenAI production key' })).toBeInTheDocument();
    expect(screen.getByRole('link', { name: 'GitHub webhook verification' })).toBeInTheDocument();
    expect(screen.getByRole('link', { name: 'Stripe webhook secret' })).toBeInTheDocument();
    expect(screen.getByRole('group', { name: 'Secrets summary' })).toBeInTheDocument();
    for (const name of [/Production/, /Webhooks/, /Staging/]) {
      expect(screen.getAllByRole('link', { name }).length).toBeGreaterThan(0);
    }
  });

  it('shows the custody tier of each vault', async () => {
    renderApp('/secrets');

    await screen.findByRole('link', { name: 'OpenAI production key' });
    expect(screen.getAllByText('Managed key').length).toBeGreaterThan(0);
    expect(screen.getAllByText('Dedicated key').length).toBeGreaterThan(0);
    expect(screen.getAllByText('Customer managed key').length).toBeGreaterThan(0);
  });

  it('never renders secret material', async () => {
    renderApp('/secrets');

    await screen.findByRole('link', { name: 'OpenAI production key' });
    expect(document.body.textContent).not.toContain('seed-openai-token');
  });

  it('filters the credentials by status through the filter bar', async () => {
    const user = userEvent.setup();
    renderApp('/secrets');

    await screen.findByRole('link', { name: 'OpenAI production key' });
    await user.click(screen.getByRole('combobox', { name: 'Filter credentials' }));
    await user.click(await screen.findByRole('option', { name: 'Status' }));
    await user.click(await screen.findByRole('option', { name: /Destroy pending/ }));

    expect(screen.queryByRole('link', { name: 'OpenAI production key' })).not.toBeInTheDocument();
    expect(screen.getByRole('link', { name: 'Old Anthropic key' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Remove Status filter' })).toBeInTheDocument();
  });

  it('offers matching values as suggestions while typing', async () => {
    const user = userEvent.setup();
    renderApp('/secrets');

    await screen.findByRole('link', { name: 'OpenAI production key' });
    await user.type(screen.getByRole('combobox', { name: 'Filter credentials' }), 'prod');
    await user.click(await screen.findByRole('option', { name: 'Vault: Production' }));

    expect(screen.getByRole('button', { name: 'Remove Vault filter' })).toBeInTheDocument();
    expect(screen.getByRole('link', { name: 'OpenAI production key' })).toBeInTheDocument();
    expect(screen.queryByRole('link', { name: 'Gemini staging key' })).not.toBeInTheDocument();
  });

  it('searches the credentials by name', async () => {
    const user = userEvent.setup();
    renderApp('/secrets');

    await screen.findByRole('link', { name: 'OpenAI production key' });
    await user.type(screen.getByRole('combobox', { name: 'Filter credentials' }), 'stripe{Enter}');

    expect(screen.getByRole('link', { name: 'Stripe webhook secret' })).toBeInTheDocument();
    expect(screen.queryByRole('link', { name: 'OpenAI production key' })).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Remove Search filter' })).toBeInTheDocument();
  });

  it('shows an empty state with a way to clear the filters', async () => {
    const user = userEvent.setup();
    renderApp('/secrets');

    await screen.findByRole('link', { name: 'OpenAI production key' });
    await user.type(screen.getByRole('combobox', { name: 'Filter credentials' }), 'zzzzzz{Enter}');

    expect(await screen.findByText('No credentials match the current filters.')).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Clear filters' }));
    expect(await screen.findByRole('link', { name: 'OpenAI production key' })).toBeInTheDocument();
  });

  it('creates a vault from the dialog', async () => {
    const user = userEvent.setup();
    renderApp('/secrets');

    await user.click(await screen.findByRole('button', { name: 'New vault' }));
    const dialog = await screen.findByRole('dialog');
    await user.type(within(dialog).getByLabelText('Vault name'), 'Sandbox');
    await user.click(within(dialog).getByRole('button', { name: 'Create vault' }));

    expect((await screen.findAllByRole('link', { name: /Sandbox/ })).length).toBeGreaterThan(0);
  });
});


it('makes vault custody chips links with tier-specific accessible names', async () => {
  renderApp('/secrets');
  await screen.findByRole('link', { name: 'OpenAI production key' });
  const cards = screen.getByRole('region', { name: 'Vaults' });
  for (const tier of ['Managed key', 'Dedicated key', 'Customer managed key']) {
    const links = within(cards).getAllByRole('link', { name: `${tier}: Encryption keys` });
    for (const link of links) expect(link).toHaveAttribute('href', '/keys');
  }
});
