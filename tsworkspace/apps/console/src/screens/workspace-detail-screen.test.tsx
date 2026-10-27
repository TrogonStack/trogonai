import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';

import { createFakeBackend } from '@/fake-backend';
import { hierarchyCommands } from '@/features/hierarchy/domain/commands';
import { HierarchyFault } from '@/features/hierarchy/domain/faults';
import { mintNodeId } from '@/features/hierarchy/domain/node-id';
import { seedNodeIds } from '@/features/hierarchy/fake/seed';
import { renderApp } from '@/test/render-app';

function optionValues(select: HTMLElement): string[] {
  return within(select)
    .getAllByRole('option')
    .map((option) => (option as HTMLOptionElement).value);
}

describe('workspace detail', () => {
  it('shows the derived word, the ancestry, the folders and the attached projects', async () => {
    renderApp('/workspaces/node_workspace?project=proj-beacon');

    expect(await screen.findByRole('heading', { name: 'Acme workspace' })).toBeInTheDocument();
    expect(screen.getByText('Workspace', { selector: 'span' })).toBeInTheDocument();
    expect(screen.queryByRole('navigation', { name: 'Ancestry' })).not.toBeInTheDocument();
    expect(await screen.findByText('Below the organization')).toBeInTheDocument();

    const folders = within(screen.getByRole('region', { name: 'Folders' }));
    expect(folders.getByRole('link', { name: /Platform folder/ })).toBeInTheDocument();
    expect(folders.getByRole('link', { name: /Research folder/ })).toBeInTheDocument();

    const projects = within(screen.getByRole('region', { name: 'Projects' }));
    const beacon = await projects.findByRole('link', { name: /Beacon/ });
    expect(beacon).toHaveAttribute('href', expect.stringContaining('/projects/proj-beacon'));
    expect(beacon).toHaveAttribute('href', expect.stringContaining('project=proj-beacon'));
  });

  it('derives the folder word and the ancestry from the depth', async () => {
    renderApp('/workspaces/node_payments');

    expect(await screen.findByRole('heading', { name: 'Payments' })).toBeInTheDocument();
    expect(screen.getByText('Folder', { selector: 'span' })).toBeInTheDocument();
    const ancestry = within(screen.getByRole('navigation', { name: 'Ancestry' }));
    expect(ancestry.getByRole('link', { name: 'Acme workspace' })).toBeInTheDocument();
    expect(ancestry.getByRole('link', { name: 'Platform folder' })).toBeInTheDocument();
  });

  it('lists the tree operations that touch the node in the activity log', async () => {
    renderApp('/workspaces/node_platform');

    const activity = within(await screen.findByRole('list', { name: 'Node activity' }));
    expect(activity.getAllByRole('listitem')).toHaveLength(2);
    expect(activity.getByText('Added')).toBeInTheDocument();
    expect(activity.getByText('Child added')).toBeInTheDocument();
  });

  it('shows a not found state for an unknown node and for the root', async () => {
    renderApp('/workspaces/nope');
    expect(await screen.findByText('Node not found')).toBeInTheDocument();
  });

  it('never opens the root as a node', async () => {
    renderApp('/workspaces/node_tenant_root');
    expect(await screen.findByText('Node not found')).toBeInTheDocument();
  });

  it('renames only the label', async () => {
    const user = userEvent.setup();
    const { hierarchy } = renderApp('/workspaces/node_payments');

    await user.click(await screen.findByRole('button', { name: 'Rename' }));
    const form = within(await screen.findByRole('form', { name: 'Rename folder' }));
    await user.clear(form.getByLabelText('Name'));
    await user.type(form.getByLabelText('Name'), 'Payment rails');
    await user.click(form.getByRole('button', { name: 'Save name' }));

    expect(await screen.findByRole('heading', { name: 'Payment rails' })).toBeInTheDocument();
    expect((await hierarchy.tree()).revision).toBe(5);
  });
});

describe('add folder', () => {
  it('adds a folder that then shows up in the new project parent picker', async () => {
    const user = userEvent.setup();
    const { hierarchy, router } = renderApp('/workspaces/node_platform');

    await user.click(await screen.findByRole('button', { name: 'Add folder' }));
    const form = within(await screen.findByRole('form', { name: 'Add folder' }));
    await user.type(form.getByLabelText('Folder name'), 'Billing');
    expect(form.getByLabelText('Folder id')).toHaveValue('billing');
    await user.click(form.getByRole('button', { name: 'Add folder' }));

    const folders = within(await screen.findByRole('region', { name: 'Folders' }));
    expect(await folders.findByRole('link', { name: /Billing/ })).toBeInTheDocument();
    expect((await hierarchy.tree()).parents.get(mintNodeId('billing'))).toBe(seedNodeIds.platform);

    await router.navigate({ to: '/projects/new' });
    const parent = await screen.findByRole('combobox', { name: 'Parent node' });
    await waitFor(() => expect(optionValues(parent)).toContain('billing'));
    expect(optionValues(parent)).not.toContain(seedNodeIds.tenantRoot);
  });

  it('refuses an id that is already taken', async () => {
    const user = userEvent.setup();
    renderApp('/workspaces/node_platform');

    await user.click(await screen.findByRole('button', { name: 'Add folder' }));
    const form = within(await screen.findByRole('form', { name: 'Add folder' }));
    await user.type(form.getByLabelText('Folder name'), 'Again');
    await user.clear(form.getByLabelText('Folder id'));
    await user.type(form.getByLabelText('Folder id'), 'billing');
    await user.click(form.getByRole('button', { name: 'Add folder' }));
    await waitFor(() => expect(screen.queryByRole('form', { name: 'Add folder' })).not.toBeInTheDocument());

    await user.click(await screen.findByRole('button', { name: 'Add folder' }));
    const again = within(await screen.findByRole('form', { name: 'Add folder' }));
    await user.type(again.getByLabelText('Folder name'), 'Again');
    await user.clear(again.getByLabelText('Folder id'));
    await user.type(again.getByLabelText('Folder id'), 'billing');
    await user.click(again.getByRole('button', { name: 'Add folder' }));

    expect(await within(await screen.findByRole('alert')).findByText('NODE_ALREADY_EXISTS')).toBeInTheDocument();
  });
});

describe('move', () => {
  it('never offers the node itself, its descendants or its current parent', async () => {
    const user = userEvent.setup();
    renderApp('/workspaces/node_platform');

    await user.click(await screen.findByRole('button', { name: 'Move' }));
    const form = within(await screen.findByRole('form', { name: 'Move node' }));
    const values = optionValues(form.getByRole('combobox', { name: 'New parent' }));
    expect(values).toEqual([seedNodeIds.tenantRoot, seedNodeIds.research]);
    expect(values).not.toContain(seedNodeIds.platform);
    expect(values).not.toContain(seedNodeIds.payments);
    expect(values).not.toContain(seedNodeIds.workspace);
  });

  it('refuses a cycle with a typed fault when it is forced through the client', async () => {
    const backend = createFakeBackend({ latencyMs: 0 });
    const { revision } = await backend.hierarchy.tree();
    const forced = backend.hierarchy.dispatch(
      hierarchyCommands.move({ nodeId: seedNodeIds.platform, parent: seedNodeIds.payments }),
      { expectedRevision: revision },
    );
    await expect(forced).rejects.toBeInstanceOf(HierarchyFault);
    await expect(forced).rejects.toMatchObject({ reason: 'NODE_CYCLE' });
  });

  it('moves a folder without a warning when its word stays the same', async () => {
    const user = userEvent.setup();
    const { hierarchy } = renderApp('/workspaces/node_payments');

    await user.click(await screen.findByRole('button', { name: 'Move' }));
    const form = within(await screen.findByRole('form', { name: 'Move node' }));
    await user.selectOptions(form.getByRole('combobox', { name: 'New parent' }), 'Research folder (folder)');
    expect(form.queryByRole('status')).not.toBeInTheDocument();
    await user.click(form.getByRole('button', { name: 'Move' }));

    await waitFor(async () =>
      expect((await hierarchy.tree()).parents.get(seedNodeIds.payments)).toBe(seedNodeIds.research),
    );
  });

  it('warns when the derived word changes and asks for the organization name on the second workspace', async () => {
    const user = userEvent.setup();
    const { hierarchy } = renderApp('/workspaces/node_payments');

    await user.click(await screen.findByRole('button', { name: 'Move' }));
    const form = within(await screen.findByRole('form', { name: 'Move node' }));
    await user.selectOptions(form.getByRole('combobox', { name: 'New parent' }), 'Organization root');

    expect(form.getByRole('status')).toHaveTextContent('changes from folder to workspace');
    expect(form.getByLabelText('Organization name')).toHaveValue('Acme workspace');
    await user.clear(form.getByLabelText('Organization name'));
    await user.type(form.getByLabelText('Organization name'), 'Acme Inc');
    await user.click(form.getByRole('button', { name: 'Move' }));

    await waitFor(async () =>
      expect((await hierarchy.tree()).parents.get(seedNodeIds.payments)).toBe(seedNodeIds.tenantRoot),
    );
    const ancestry = within(await screen.findByRole('navigation', { name: 'Ancestry' }));
    expect(await ancestry.findByText('Acme Inc')).toBeInTheDocument();
    expect(ancestry.queryByRole('link')).not.toBeInTheDocument();
  });
});

describe('remove', () => {
  it('is refused with the reason while the node has children or attached projects', async () => {
    renderApp('/workspaces/node_platform');

    const remove = await screen.findByRole('button', { name: 'Remove' });
    expect(remove).toBeDisabled();
    expect(screen.getByText(/1 child node is still below it/)).toBeInTheDocument();
    expect(screen.getByText(/1 project is still attached/)).toBeInTheDocument();
  });

  it('is refused while only a project is attached', async () => {
    renderApp('/workspaces/node_research');

    expect(await screen.findByRole('button', { name: 'Remove' })).toBeDisabled();
    expect(await screen.findByText(/1 project is still attached/)).toBeInTheDocument();
  });

  it('is refused by the client with a typed fault when a project is attached', async () => {
    const backend = createFakeBackend({ latencyMs: 0 });
    const { revision } = await backend.hierarchy.tree();
    const forced = backend.hierarchy.dispatch(hierarchyCommands.remove(seedNodeIds.research), {
      expectedRevision: revision,
    });
    await expect(forced).rejects.toMatchObject({ reason: 'NODE_HAS_ATTACHMENTS' });
  });

  it('removes the empty folder after confirmation and drops it from the parent picker', async () => {
    const user = userEvent.setup();
    const { hierarchy, router } = renderApp('/workspaces/node_payments');

    await user.click(await screen.findByRole('button', { name: 'Remove' }));
    await user.click(await screen.findByRole('button', { name: 'Remove folder' }));

    await waitFor(() => expect(router.state.location.pathname).toBe('/workspaces/node_platform'));
    expect((await hierarchy.tree()).parents.has(seedNodeIds.payments)).toBe(false);

    await router.navigate({ to: '/projects/new' });
    const parent = await screen.findByRole('combobox', { name: 'Parent node' });
    await waitFor(() => expect(optionValues(parent)).not.toContain(seedNodeIds.payments));
  });

  it('keeps the removed folder in the activity log of its parent', async () => {
    const user = userEvent.setup();
    renderApp('/workspaces/node_payments');

    await user.click(await screen.findByRole('button', { name: 'Remove' }));
    await user.click(await screen.findByRole('button', { name: 'Remove folder' }));

    const activity = within(await screen.findByRole('list', { name: 'Node activity' }));
    expect(await activity.findByText('Child removed')).toBeInTheDocument();
    expect(activity.getAllByText('Payments').length).toBeGreaterThan(0);
  });
});
