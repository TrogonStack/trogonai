import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { BindingReplacementMode } from '@trogonai/proto';
import { describe, expect, it } from 'vitest';

import { replacementModeLabels } from '@/features/provider-connections/domain/labels';
import { renderApp } from '@/test/render-app';

async function openTab(user: ReturnType<typeof userEvent.setup>, name: RegExp) {
  await user.click(await screen.findByRole('tab', { name }));
}

describe('provider detail', () => {
  it('shows the immutable configuration, digest, bindings with pinned sessions and history', async () => {
    const user = userEvent.setup();
    renderApp('/providers/openai-prod');

    expect(await screen.findByRole('heading', { name: 'OpenAI production' })).toBeInTheDocument();
    expect(screen.getByText(/Changing any of it means registering a new connection/)).toBeInTheDocument();
    expect(screen.getByText('Configuration digest')).toBeInTheDocument();
    expect(screen.getAllByTitle(/sha256:[0-9a-f]{64}/).length).toBeGreaterThan(0);
    expect(screen.getByText('Immutable')).toBeInTheDocument();

    await openTab(user, /^Bindings/);
    const bindings = screen.getByRole('list', { name: 'Credential bindings' });
    expect(within(bindings).getByText('openai-prod-key-3')).toBeInTheDocument();
    expect(within(bindings).getByText('Draining')).toBeInTheDocument();
    expect(within(bindings).getByText('3')).toBeInTheDocument();
    expect(
      within(bindings)
        .getAllByRole('meter', { name: 'Pinned sessions' })
        .map((meter) => meter.getAttribute('aria-valuenow')),
    ).toContain('3');
    expect(screen.queryByRole('button', { name: 'Activate' })).not.toBeInTheDocument();

    await openTab(user, /^History/);
    const history = await screen.findByRole('list', { name: 'Event history' });
    expect(within(history).getByText('Registered')).toBeInTheDocument();
    expect(within(history).getAllByText('Binding replaced')).toHaveLength(2);
  });

  it('activates a pending connection', async () => {
    const user = userEvent.setup();
    renderApp('/providers/anthropic-eval');

    await user.click(await screen.findByRole('button', { name: 'Activate' }));

    await waitFor(() => expect(screen.queryByRole('button', { name: 'Activate' })).not.toBeInTheDocument());
    expect(screen.getAllByText('Active').length).toBeGreaterThan(0);
    expect(await screen.findByText('Connection activated')).toBeInTheDocument();
    await openTab(user, /^History/);
    expect(await screen.findByText('Activated')).toBeInTheDocument();
  });

  it('surfaces the decider fault when replacing a binding that live Sessions still pin', async () => {
    const user = userEvent.setup();
    renderApp('/providers/openai-prod');

    await user.click(await screen.findByRole('button', { name: 'Replace binding' }));
    const dialog = await screen.findByRole('dialog');
    await user.type(within(dialog).getByLabelText('Binding id'), 'openai-prod-key-4');
    await user.type(within(dialog).getByLabelText('Secret reference'), 'secret://providers/openai-prod/key-4');
    await user.click(
      within(dialog).getByRole('radio', {
        name: new RegExp(replacementModeLabels[BindingReplacementMode.FAIL_IF_IN_USE].split(':')[0]!),
      }),
    );
    await user.click(within(dialog).getByRole('button', { name: 'Replace binding' }));

    expect(await within(dialog).findByText('CREDENTIAL_BINDING_IN_USE')).toBeInTheDocument();
  });

  it('replaces a binding in drain mode', async () => {
    const user = userEvent.setup();
    renderApp('/providers/openai-prod');

    await user.click(await screen.findByRole('button', { name: 'Replace binding' }));
    const dialog = await screen.findByRole('dialog');
    await user.type(within(dialog).getByLabelText('Binding id'), 'openai-prod-key-4');
    await user.type(within(dialog).getByLabelText('Secret reference'), 'secret://providers/openai-prod/key-4');
    await user.click(within(dialog).getByRole('button', { name: 'Replace binding' }));

    await waitFor(() => expect(screen.queryByRole('dialog', { name: /Replace binding/ })).not.toBeInTheDocument());
    await openTab(user, /^Bindings/);
    const bindings = await screen.findByRole('list', { name: 'Credential bindings' });
    expect(await within(bindings).findByText('openai-prod-key-4')).toBeInTheDocument();
  });

  it('revokes a connection after confirmation', async () => {
    const user = userEvent.setup();
    renderApp('/providers/ollama-local');

    await user.click(await screen.findByRole('button', { name: 'Connection actions' }));
    await user.click(await screen.findByRole('menuitem', { name: 'Revoke connection' }));
    const dialog = await screen.findByRole('alertdialog');
    await user.click(within(dialog).getByRole('button', { name: 'Revoke connection' }));

    await waitFor(() => expect(screen.queryByRole('button', { name: 'Connection actions' })).not.toBeInTheDocument());
    expect(screen.getAllByText('Revoked').length).toBeGreaterThan(0);
  });

  it('saves the profile only when it has changed', async () => {
    const user = userEvent.setup();
    renderApp('/providers/openai-prod');

    const save = await screen.findByRole('button', { name: 'Save profile' });
    expect(save).toBeDisabled();
    await user.type(screen.getByLabelText('Display name'), ' EU');
    expect(save).toBeEnabled();
    await user.click(save);

    expect(await screen.findByText('Profile saved')).toBeInTheDocument();
    await waitFor(() => expect(screen.getByRole('button', { name: 'Save profile' })).toBeDisabled());
    expect(await screen.findByRole('heading', { name: 'OpenAI production EU' })).toBeInTheDocument();
  });

  it('keeps the routing policy save disabled until the preference changes', async () => {
    const user = userEvent.setup();
    renderApp('/providers/openai-prod');

    const save = await screen.findByRole('button', { name: 'Save routing policy' });
    expect(save).toBeDisabled();
    await user.type(screen.getByLabelText('Routing preference'), '5');
    expect(save).toBeEnabled();
  });

  it('reports an unknown connection', async () => {
    renderApp('/providers/does-not-exist');
    expect(await screen.findByRole('heading', { name: 'Connection not found' })).toBeInTheDocument();
  });
});
