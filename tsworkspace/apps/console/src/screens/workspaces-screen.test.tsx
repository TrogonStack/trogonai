import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';

import { createFakeBackend, type FakeBackend } from '@/fake-backend';
import { hierarchyCommands } from '@/features/hierarchy/domain/commands';
import { mintNodeId } from '@/features/hierarchy/domain/node-id';
import { mintNodeName } from '@/features/hierarchy/domain/node-name';
import { seedNodeIds } from '@/features/hierarchy/fake/seed';
import { renderApp } from '@/test/render-app';

const second = mintNodeId('second');

async function backendWithSecondWorkspace(): Promise<FakeBackend> {
  const backend = createFakeBackend({ latencyMs: 0 });
  const { revision } = await backend.hierarchy.tree();
  await backend.hierarchy.dispatch(hierarchyCommands.add({ nodeId: second, parent: seedNodeIds.tenantRoot }), {
    expectedRevision: revision,
  });
  await backend.hierarchy.setLabel(second, mintNodeName('Second workspace'));
  await backend.hierarchy.setLabel(seedNodeIds.tenantRoot, mintNodeName('Acme Inc'));
  return backend;
}

async function openForm() {
  return within(await screen.findByRole('form', { name: 'Create workspace' }));
}

describe('workspaces list', () => {
  it('lists the workspace with its folders and projects and keeps the organization hidden', async () => {
    renderApp('/workspaces');

    expect(await screen.findByRole('heading', { name: 'Workspaces' })).toBeInTheDocument();
    const row = (await screen.findByRole('link', { name: 'Acme workspace' })).closest('tr')!;
    expect(within(row).getByText('node_workspace')).toBeInTheDocument();
    const cells = within(row).getAllByRole('cell');
    expect(cells[1]).toHaveTextContent('3');
    expect(cells[2]).toHaveTextContent('3');
    expect(screen.queryByText(/Organization \/ Acme/)).not.toBeInTheDocument();
    const nav = screen.getByRole('navigation', { name: 'Primary' });
    expect(await within(nav).findByRole('link', { name: /Workspaces/ })).toHaveTextContent('01');
  });

  it('shows the organization in the header and the breadcrumbs once a second workspace exists', async () => {
    renderApp('/workspaces', await backendWithSecondWorkspace());

    expect(await screen.findByText('Organization / Acme Inc')).toBeInTheDocument();
    expect(screen.getByRole('link', { name: 'Second workspace' })).toBeInTheDocument();
    const crumbs = within(screen.getByRole('navigation', { name: 'Breadcrumb' }));
    expect(await crumbs.findByText('Acme Inc')).toBeInTheDocument();
  });

  it('hides the organization again when the workspaces drop back to one', async () => {
    const user = userEvent.setup();
    const backend = await backendWithSecondWorkspace();
    renderApp(`/workspaces/${second}`, backend);

    await user.click(await screen.findByRole('button', { name: 'Remove' }));
    await user.click(await screen.findByRole('button', { name: 'Remove workspace' }));

    await screen.findByRole('link', { name: 'Acme workspace' });
    expect(screen.queryByText(/Organization \/ Acme Inc/)).not.toBeInTheDocument();
    const labels = await backend.hierarchy.labels();
    expect(labels.find((entry) => entry.nodeId === seedNodeIds.tenantRoot)?.name).toBe('Acme Inc');
  });

  it('filters workspaces by text and clears the filter', async () => {
    const user = userEvent.setup();
    renderApp('/workspaces', await backendWithSecondWorkspace());

    await screen.findByRole('link', { name: 'Second workspace' });
    await user.type(screen.getByRole('combobox', { name: 'Filter workspaces' }), 'second{Enter}');
    await waitFor(() => expect(screen.queryByRole('link', { name: 'Acme workspace' })).not.toBeInTheDocument());

    await user.clear(screen.getByRole('combobox', { name: 'Filter workspaces' }));
    await user.type(screen.getByRole('combobox', { name: 'Filter workspaces' }), 'zzzz{Enter}');
    expect(await screen.findByText('No matches')).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Clear filters' }));
    expect(await screen.findByRole('link', { name: 'Acme workspace' })).toBeInTheDocument();
  });
});

describe('new workspace', () => {
  it('suggests an editable id from the name', async () => {
    const user = userEvent.setup();
    renderApp('/workspaces/new');
    const form = await openForm();

    await user.type(form.getByLabelText('Workspace name'), 'Payments Hub');
    expect(form.getByLabelText('Workspace id')).toHaveValue('payments-hub');
  });

  it('asks for the organization name on the second workspace, defaulting to the first workspace name', async () => {
    const user = userEvent.setup();
    const { hierarchy, router } = renderApp('/workspaces/new');
    const form = await openForm();

    expect(form.getByLabelText('Organization name')).toHaveValue('Acme workspace');
    await user.type(form.getByLabelText('Workspace name'), 'Payments Hub');
    await user.clear(form.getByLabelText('Organization name'));
    await user.type(form.getByLabelText('Organization name'), 'Acme Inc');
    await user.click(form.getByRole('button', { name: 'Create workspace' }));

    await waitFor(() => expect(router.state.location.pathname).toBe('/workspaces/payments-hub'));
    const tree = await hierarchy.tree();
    expect(tree.parents.get(mintNodeId('payments-hub'))).toBe(seedNodeIds.tenantRoot);
    const labels = await hierarchy.labels();
    expect(labels.find((entry) => entry.nodeId === seedNodeIds.tenantRoot)?.name).toBe('Acme Inc');
    expect(labels.find((entry) => entry.nodeId === mintNodeId('payments-hub'))?.name).toBe('Payments Hub');
    const crumbs = within(await screen.findByRole('navigation', { name: 'Breadcrumb' }));
    expect(await crumbs.findByText('Acme Inc')).toBeInTheDocument();
  });

  it('does not ask for the organization name again on the third workspace', async () => {
    renderApp('/workspaces/new', await backendWithSecondWorkspace());
    const form = await openForm();
    expect(form.queryByLabelText('Organization name')).not.toBeInTheDocument();
  });

  it('validates the name, the id and the organization name', async () => {
    const user = userEvent.setup();
    const { hierarchy } = renderApp('/workspaces/new');
    const form = await openForm();

    await user.clear(form.getByLabelText('Organization name'));
    await user.type(form.getByLabelText('Workspace id'), 'Not Valid');
    await user.click(form.getByRole('button', { name: 'Create workspace' }));

    expect(await form.findAllByText('A name is required.')).toHaveLength(2);
    expect(form.getByText('An id starts with a lowercase letter.')).toBeInTheDocument();
    expect((await hierarchy.tree()).revision).toBe(5);
  });

  it('reports a duplicate id as a typed fault', async () => {
    const user = userEvent.setup();
    renderApp('/workspaces/new', await backendWithSecondWorkspace());
    const form = await openForm();

    await user.type(form.getByLabelText('Workspace name'), 'Second');
    await user.click(form.getByRole('button', { name: 'Create workspace' }));

    const alert = await screen.findByRole('alert');
    expect(within(alert).getByText('The workspace was rejected')).toBeInTheDocument();
    expect(within(alert).getByText('NODE_ALREADY_EXISTS')).toBeInTheDocument();
  });
});
