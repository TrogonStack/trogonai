import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { ProjectAccessPreset } from '@trogonai/proto';
import { describe, expect, it } from 'vitest';

import { seedNodeIds } from '@/features/projects/fake/nodes';
import { renderApp } from '@/test/render-app';

async function openForm() {
  const form = await screen.findByRole('form', { name: 'Create project' });
  return within(form);
}

describe('new project', () => {
  it('suggests an editable id from the name', async () => {
    const user = userEvent.setup();
    renderApp('/projects/new');
    const form = await openForm();

    await user.type(form.getByLabelText('Project name'), 'Billing Revamp');
    expect(form.getByLabelText('Project id')).toHaveValue('billing-revamp');

    await user.clear(form.getByLabelText('Project id'));
    await user.type(form.getByLabelText('Project id'), 'billing-2');
    await user.type(form.getByLabelText('Project name'), '!');
    expect(form.getByLabelText('Project id')).toHaveValue('billing-2');
  });

  it('never offers the tenant root as a parent', async () => {
    renderApp('/projects/new');
    const form = await openForm();

    const parent = form.getByRole('combobox', { name: 'Parent node' });
    const values = within(parent)
      .getAllByRole('option')
      .map((option) => (option as HTMLOptionElement).value);
    expect(values).toEqual(['node_workspace', 'node_platform', 'node_research']);
    expect(values).not.toContain(seedNodeIds.tenantRoot);
  });

  it('requires an explicit access preset and explains each one', async () => {
    const user = userEvent.setup();
    const { projects } = renderApp('/projects/new');
    const form = await openForm();

    expect(form.getByRole('radio', { name: /Inherited/ })).not.toBeChecked();
    expect(form.getByRole('radio', { name: /Members only/ })).not.toBeChecked();
    expect(form.getByText(/Whoever the parent level lets in can work here/)).toBeInTheDocument();
    expect(form.getByText(/Only people who hold a project membership get in/)).toBeInTheDocument();

    await user.type(form.getByLabelText('Project name'), 'Billing');
    await user.type(form.getByLabelText('Goal'), 'Keep invoices flowing.');
    await user.click(form.getByRole('button', { name: 'Create project' }));

    expect(await form.findByText('Choose an access preset. It is fixed once the project exists.')).toBeInTheDocument();
    expect(await projects.get('billing')).toBeUndefined();
  });

  it('rejects an invalid id before dispatching', async () => {
    const user = userEvent.setup();
    const { projects } = renderApp('/projects/new');
    const form = await openForm();

    await user.type(form.getByLabelText('Project name'), 'Billing');
    await user.clear(form.getByLabelText('Project id'));
    await user.type(form.getByLabelText('Project id'), 'Not Valid');
    await user.type(form.getByLabelText('Goal'), 'Keep invoices flowing.');
    await user.click(form.getByRole('radio', { name: /Inherited/ }));
    await user.click(form.getByRole('button', { name: 'Create project' }));

    expect(await form.findByText('A project id starts with a lowercase letter.')).toBeInTheDocument();
    expect(await projects.list()).toHaveLength(3);
  });

  it('creates the project with the chosen preset and opens it', async () => {
    const user = userEvent.setup();
    const { projects, router } = renderApp('/projects/new');
    const form = await openForm();

    await user.type(form.getByLabelText('Project name'), 'Billing Revamp');
    await user.type(form.getByLabelText('Goal'), 'Keep invoices flowing.');
    await user.selectOptions(form.getByRole('combobox', { name: 'Parent node' }), 'Platform folder');
    await user.click(form.getByRole('radio', { name: /Members only/ }));
    await user.click(form.getByRole('button', { name: 'Create project' }));

    await waitFor(() => expect(router.state.location.pathname).toBe('/projects/billing-revamp'));
    const created = await projects.get('billing-revamp');
    expect(created?.project.initialAccessPreset).toBe(ProjectAccessPreset.MEMBERS_ONLY);
    expect(created?.project.parent?.value).toBe('node_platform');
    expect(await screen.findByRole('heading', { name: 'Billing Revamp' })).toBeInTheDocument();
  });

  it('reports a duplicate id', async () => {
    const user = userEvent.setup();
    renderApp('/projects/new');
    const form = await openForm();

    await user.type(form.getByLabelText('Project name'), 'Beacon again');
    await user.clear(form.getByLabelText('Project id'));
    await user.type(form.getByLabelText('Project id'), 'proj-beacon');
    await user.type(form.getByLabelText('Goal'), 'Duplicate.');
    await user.click(form.getByRole('radio', { name: /Inherited/ }));
    await user.click(form.getByRole('button', { name: 'Create project' }));

    const alert = await screen.findByRole('alert');
    expect(within(alert).getByText('The project was rejected')).toBeInTheDocument();
  });
});
