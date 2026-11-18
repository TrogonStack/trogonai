import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';

import { renderApp } from '@/test/render-app';

describe('generic webhook management preview', () => {
  it('lists generic webhooks with providers, with setup and delivery controls only on the detail page', async () => {
    const user = userEvent.setup();
    const { router } = renderApp('/sources');
    const link = await screen.findByRole('link', { name: 'Orders service' });
    const table = screen.getByRole('table');
    expect(within(table).getByRole('link', { name: 'GitHub local' })).toBeInTheDocument();
    expect(within(table).getByRole('link', { name: 'Orders service' })).toBe(link);
    const row = link.closest('tr')!;
    expect(within(row).getByText('Generic webhook')).toBeInTheDocument();
    expect(within(row).getByText('1 runtime')).toBeInTheDocument();
    expect(within(row).getByText('Webhook secret · v1')).toBeInTheDocument();
    expect(within(row).getByText('Never')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Simulate delivery' })).not.toBeInTheDocument();
    expect(screen.queryByText('Example endpoint URL')).not.toBeInTheDocument();
    expect(screen.queryByRole('heading', { name: 'Provider sources' })).not.toBeInTheDocument();
    await user.click(link);
    expect(await screen.findByRole('heading', { name: 'Orders service', level: 1 })).toBeInTheDocument();
    expect(router.state.location.pathname).toMatch(/^\/sources\/webhooks\/[0-9a-f-]+$/);
    expect(screen.getByRole('button', { name: 'Simulate delivery' })).toBeInTheDocument();
    await user.click(screen.getByRole('link', { name: 'All sources' }));
    expect(await screen.findByRole('link', { name: 'GitHub local' })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Simulate delivery' })).not.toBeInTheDocument();
  });

  it('filters the shared source table by the Generic webhook kind', async () => {
    const user = userEvent.setup();
    renderApp('/sources');
    await screen.findByRole('link', { name: 'Orders service' });
    await user.click(screen.getByRole('combobox', { name: 'Filter sources' }));
    await user.click(await screen.findByRole('option', { name: 'Kind' }));
    await user.click(await screen.findByRole('option', { name: /Generic webhook/ }));
    expect(screen.getByRole('link', { name: 'Orders service' })).toBeInTheDocument();
    expect(screen.queryByRole('link', { name: 'GitHub local' })).not.toBeInTheDocument();
  });

  it('renders a normal missing-source state for an unknown endpoint identifier', async () => {
    renderApp('/sources/webhooks/not-a-uuid');
    expect(await screen.findByRole('heading', { name: 'Source not found' })).toBeInTheDocument();
    expect(screen.getByRole('link', { name: 'All sources' })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Simulate delivery' })).not.toBeInTheDocument();
  });

  it('creates a bearer endpoint with an example URL and keeps setup inside the selected project', async () => {
    const user = userEvent.setup();
    renderApp('/sources');
    await screen.findByRole('link', { name: 'Orders service' });
    await user.click(screen.getByRole('button', { name: 'Create webhook' }));
    const dialog = await screen.findByRole('dialog');
    await user.type(within(dialog).getByRole('textbox', { name: 'Endpoint name' }), 'Billing events');
    await user.click(within(dialog).getByRole('radio', { name: /Bearer token/ }));
    expect(
      within(dialog).queryByRole('spinbutton', { name: 'Timestamp tolerance in seconds' }),
    ).not.toBeInTheDocument();
    await user.click(within(dialog).getByRole('button', { name: 'Create endpoint' }));
    expect(await screen.findByRole('heading', { name: 'Billing events' })).toBeInTheDocument();
    expect(
      screen.getByText(
        /^https:\/\/gateway\.example\.com\/webhooks\/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/,
      ),
    ).toBeInTheDocument();
    expect(screen.getByText(/not connected to a running gateway/)).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: /Switch project/ }));
    await user.click(await screen.findByRole('button', { name: 'Beacon' }));
    expect(await screen.findByText('No integrations yet')).toBeInTheDocument();
    expect(screen.queryByRole('heading', { name: 'Billing events' })).not.toBeInTheDocument();
  });

  it('shows validation errors and never creates an endpoint with an invalid payload limit', async () => {
    const user = userEvent.setup();
    renderApp('/sources');
    await screen.findByRole('link', { name: 'Orders service' });
    await user.click(screen.getByRole('button', { name: 'Create webhook' }));
    const dialog = await screen.findByRole('dialog');
    await user.type(within(dialog).getByRole('textbox', { name: 'Endpoint name' }), 'Too large');
    const limit = within(dialog).getByRole('spinbutton', { name: 'Maximum body in bytes' });
    await user.clear(limit);
    await user.type(limit, '1048577');
    await user.click(within(dialog).getByRole('button', { name: 'Create endpoint' }));
    expect(await within(dialog).findByText('Enter a whole number from 1 to 1048576 bytes.')).toBeInTheDocument();
    expect(screen.queryByRole('heading', { name: 'Too large' })).not.toBeInTheDocument();
  });

  it('simulates receipt, retry, pause and revocation without claiming business completion', async () => {
    const user = userEvent.setup();
    renderApp('/sources');
    await user.click(await screen.findByRole('link', { name: 'Orders service' }));
    await screen.findByRole('heading', { name: 'Orders service' });
    const simulate = () => user.click(screen.getByRole('button', { name: 'Simulate delivery' }));
    await simulate();
    expect(await screen.findByText('One WebhookReceived event.')).toBeInTheDocument();
    await simulate();
    expect(await screen.findByText('The original event is retained.')).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Pause endpoint' }));
    await user.click(within(screen.getByRole('alertdialog')).getByRole('button', { name: 'Pause endpoint' }));
    await screen.findByRole('button', { name: 'Enable endpoint' });
    await simulate();
    expect(await screen.findByText('Simulated 404')).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Enable endpoint' }));
    await screen.findByRole('button', { name: 'Pause endpoint' });
    await user.click(screen.getByRole('button', { name: 'Revoke' }));
    await user.click(within(screen.getByRole('alertdialog')).getByRole('button', { name: 'Revoke credential' }));
    await waitFor(() => expect(screen.getByRole('button', { name: 'Rotate' })).toBeDisabled());
    await simulate();
    expect(await screen.findByText('Simulated 401')).toBeInTheDocument();
    const deliveries = screen.getByRole('list', { name: 'Recent webhook deliveries' });
    expect(within(deliveries).getAllByRole('listitem')).toHaveLength(4);
    expect(screen.getByText(/no business action runs/)).toBeInTheDocument();
  });

  it('edits an endpoint on its detail page and returns to the shared table after deletion', async () => {
    const user = userEvent.setup();
    const { router } = renderApp('/sources');
    await user.click(await screen.findByRole('link', { name: 'Orders service' }));
    await user.click(await screen.findByRole('button', { name: 'Settings' }));
    const dialog = await screen.findByRole('dialog');
    const name = within(dialog).getByRole('textbox', { name: 'Endpoint name' });
    await user.clear(name);
    await user.type(name, 'Orders webhook');
    await user.click(within(dialog).getByRole('button', { name: 'Save changes' }));
    await screen.findByRole('heading', { name: 'Orders webhook', level: 1 });
    await user.click(screen.getByRole('button', { name: 'Delete endpoint' }));
    await user.click(within(screen.getByRole('alertdialog')).getByRole('button', { name: 'Delete endpoint' }));
    await waitFor(() => expect(router.state.location.pathname).toBe('/sources'));
    expect(await screen.findByRole('link', { name: 'GitHub local' })).toBeInTheDocument();
    expect(screen.queryByRole('link', { name: 'Orders webhook' })).not.toBeInTheDocument();
  });
});
