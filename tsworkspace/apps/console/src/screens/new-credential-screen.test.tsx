import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';

import { renderApp } from '@/test/render-app';

type User = ReturnType<typeof userEvent.setup>;

async function fillWebhook(user: User, name: string) {
  const form = await screen.findByRole('form', { name: 'Add credential' });
  await user.type(within(form).getByLabelText('Display name'), name);
  await user.click(within(form).getByRole('radio', { name: /Webhook verification/ }));
  return form;
}

describe('add credential', () => {
  it('reveals a generated webhook secret exactly once and never on the detail page', async () => {
    const user = userEvent.setup();
    const { router } = renderApp('/secrets/new');

    const form = await fillWebhook(user, 'Linear webhook');
    await user.click(within(form).getByRole('button', { name: /Generate/ }));
    await user.click(within(form).getByRole('button', { name: 'Create credential' }));

    const revealed = await screen.findByTestId('revealed-secret');
    expect(screen.getByRole('region', { name: 'Generated secret' })).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: /Show/ }));
    const value = revealed.textContent ?? '';
    expect(value.length).toBeGreaterThan(16);
    expect(value).not.toContain('•');

    await user.click(screen.getByRole('button', { name: 'I have stored it' }));

    await waitFor(() => expect(router.state.location.pathname).toMatch(/^\/secrets\/credentials\//));
    expect(await screen.findByRole('heading', { name: 'Linear webhook' })).toBeInTheDocument();
    expect(screen.queryByTestId('revealed-secret')).not.toBeInTheDocument();
    expect(document.body.textContent).not.toContain(value);
  });

  it('never echoes an imported secret', async () => {
    const user = userEvent.setup();
    const { router } = renderApp('/secrets/new');

    const form = await fillWebhook(user, 'Imported webhook');
    await user.type(within(form).getByLabelText('Secret material'), 'whsec-imported-material-123');
    await user.click(within(form).getByRole('button', { name: 'Create credential' }));

    expect(await screen.findByRole('heading', { name: 'Imported webhook' })).toBeInTheDocument();
    expect(router.state.location.pathname).toMatch(/^\/secrets\/credentials\//);
    expect(screen.queryByTestId('revealed-secret')).not.toBeInTheDocument();
    expect(document.body.textContent).not.toContain('whsec-imported-material-123');
  });

  it('asks for material when importing', async () => {
    const user = userEvent.setup();
    renderApp('/secrets/new');

    const form = await fillWebhook(user, 'Missing material');
    await user.click(within(form).getByRole('button', { name: 'Create credential' }));

    expect(await within(form).findByText('The secret material is required.')).toBeInTheDocument();
  });
});
