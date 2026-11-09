import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';

import { renderApp } from '@/test/render-app';

describe('providers list', () => {
  it('renders the seeded connections with lifecycle, scope and binding summary', async () => {
    renderApp('/providers');

    const table = await screen.findByRole('table');
    expect(await within(table).findByText('OpenAI production')).toBeInTheDocument();
    expect(within(table).getByText('openai-prod')).toBeInTheDocument();
    expect(within(table).getByText('api.openai.com')).toHaveAttribute('title', 'https://api.openai.com/v1');
    expect(within(table).getAllByText('Active').length).toBeGreaterThan(0);
    expect(within(table).getAllByText('Revoked').length).toBeGreaterThan(0);
    expect(within(table).getAllByText('Pending').length).toBeGreaterThan(0);
    expect(within(table).getByText('local · ollama-local')).toBeInTheDocument();
    expect(screen.getByRole('link', { name: 'New connection' })).toHaveAttribute('href', '/providers/new');
    expect(
      within(screen.getByRole('navigation', { name: 'Primary' })).getByRole('link', { name: /^Providers/ }),
    ).toBeInTheDocument();
  });

  it('filters by lifecycle and search', async () => {
    const user = userEvent.setup();
    renderApp('/providers');
    const table = await screen.findByRole('table');
    await within(table).findByText('OpenAI production');

    await user.click(screen.getByRole('combobox', { name: 'Filter connections' }));
    await user.click(await screen.findByRole('option', { name: 'Status' }));
    await user.click(await screen.findByRole('option', { name: /Revoked/ }));
    await waitFor(() => expect(within(table).queryByText('OpenAI production')).not.toBeInTheDocument());
    expect(within(table).getByText('gemini-legacy')).toBeInTheDocument();

    await user.click(screen.getByRole('button', { name: 'Clear all filters' }));
    await user.type(screen.getByRole('combobox', { name: 'Filter connections' }), 'ollama{Enter}');
    await waitFor(() => expect(within(table).queryByText('OpenAI production')).not.toBeInTheDocument());
    expect(within(table).getByText('ollama-local')).toBeInTheDocument();
  });

  it('redirects the home route to the providers list', async () => {
    renderApp('/');
    expect(await screen.findByRole('heading', { name: 'Provider connections' })).toBeInTheDocument();
  });

  it('summarises the fleet in the stat cards', async () => {
    renderApp('/providers');
    const summary = await screen.findByRole('group', { name: 'Connection summary' });
    expect(within(summary).getByText('Pending validation')).toBeInTheDocument();
    expect(within(summary).getByText('Draining bindings')).toBeInTheDocument();
  });
});
