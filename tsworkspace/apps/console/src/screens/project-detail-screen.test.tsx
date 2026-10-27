import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';

import { renderApp } from '@/test/render-app';

describe('project detail', () => {
  it('shows the goal, immutable genesis facts, owned resources and the activity log', async () => {
    renderApp('/projects/proj-demo');

    expect(await screen.findByRole('heading', { name: 'Customer portal' })).toBeInTheDocument();
    expect(
      screen.getByText('Ship the customer portal and keep its credentials rotated.', { selector: 'p.max-w-prose' }),
    ).toBeInTheDocument();

    const genesis = screen.getByRole('region', { name: 'Genesis facts' });
    expect(within(genesis).getByText('Immutable')).toBeInTheDocument();
    expect(await within(genesis).findByText('Platform folder')).toBeInTheDocument();
    expect(within(genesis).getByText('Inherited')).toBeInTheDocument();

    const resources = await screen.findByRole('region', { name: 'Owned resources' });
    expect(within(resources).getByRole('link', { name: 'Open secrets' })).toHaveAttribute(
      'href',
      '/secrets?project=proj-demo',
    );
    expect(within(resources).getByRole('link', { name: 'Open environments' })).toHaveAttribute(
      'href',
      '/environments?project=proj-demo',
    );

    const activity = screen.getByRole('list', { name: 'Project activity' });
    const entries = within(activity).getAllByRole('listitem');
    expect(entries).toHaveLength(3);
    expect(within(entries[0]!).getByText('Renamed')).toBeInTheDocument();
    expect(within(entries[1]!).getByText('Goal restated')).toBeInTheDocument();
    expect(within(entries[2]!).getByText('Created')).toBeInTheDocument();
  });

  it('shows not found for an unknown project', async () => {
    renderApp('/projects/proj-missing');

    expect(await screen.findByText('Project not found')).toBeInTheDocument();
    expect(screen.getByRole('link', { name: /All projects/ })).toBeInTheDocument();
  });

  it('enables every action on an active project', async () => {
    renderApp('/projects/proj-beacon');

    await screen.findByRole('heading', { name: 'Beacon' });
    expect(screen.getByRole('button', { name: 'Rename' })).toBeEnabled();
    expect(screen.getByRole('button', { name: 'Restate goal' })).toBeEnabled();
    expect(screen.getByRole('button', { name: 'Archive' })).toBeEnabled();
    expect(screen.queryByRole('button', { name: 'Unarchive' })).not.toBeInTheDocument();
  });

  it('allows only unarchive on an archived project and says why', async () => {
    renderApp('/projects/proj-atlas');

    await screen.findByRole('heading', { name: 'Atlas migration' });
    expect(screen.getByRole('button', { name: 'Rename' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Restate goal' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Unarchive' })).toBeEnabled();
    expect(screen.queryByRole('button', { name: 'Archive' })).not.toBeInTheDocument();
    expect(screen.getByText(/archived/i, { selector: 'p.text-right' })).toBeInTheDocument();
  });

  it('renames a project and records it in the activity log', async () => {
    const user = userEvent.setup();
    renderApp('/projects/proj-beacon');

    await user.click(await screen.findByRole('button', { name: 'Rename' }));
    const form = within(await screen.findByRole('form', { name: 'Rename project' }));
    await user.clear(form.getByLabelText('Project name'));
    await user.type(form.getByLabelText('Project name'), 'Beacon edge');
    await user.click(form.getByRole('button', { name: 'Save name' }));

    expect(await screen.findByRole('heading', { name: 'Beacon edge' })).toBeInTheDocument();
    await waitFor(() =>
      expect(within(screen.getByRole('list', { name: 'Project activity' })).getByText('Renamed')).toBeInTheDocument(),
    );
  });

  it('restores the saved name when the rename dialog is reopened after a cancel', async () => {
    const user = userEvent.setup();
    renderApp('/projects/proj-beacon');

    await user.click(await screen.findByRole('button', { name: 'Rename' }));
    let form = within(await screen.findByRole('form', { name: 'Rename project' }));
    const original = (form.getByLabelText('Project name') as HTMLInputElement).value;
    await user.type(form.getByLabelText('Project name'), ' discarded');
    await user.click(form.getByRole('button', { name: 'Cancel' }));
    await waitFor(() => expect(screen.queryByRole('form', { name: 'Rename project' })).not.toBeInTheDocument());

    await user.click(screen.getByRole('button', { name: 'Rename' }));
    form = within(await screen.findByRole('form', { name: 'Rename project' }));
    expect(form.getByLabelText('Project name')).toHaveValue(original);
  });

  it('refuses an empty name without dispatching', async () => {
    const user = userEvent.setup();
    const { projects } = renderApp('/projects/proj-beacon');

    await user.click(await screen.findByRole('button', { name: 'Rename' }));
    const form = within(await screen.findByRole('form', { name: 'Rename project' }));
    await user.clear(form.getByLabelText('Project name'));
    await user.click(form.getByRole('button', { name: 'Save name' }));

    expect(await form.findByRole('alert')).toBeInTheDocument();
    expect((await projects.get('proj-beacon'))?.revision).toBe(1);
  });

  it('restates the goal', async () => {
    const user = userEvent.setup();
    renderApp('/projects/proj-beacon');

    await user.click(await screen.findByRole('button', { name: 'Restate goal' }));
    const form = within(await screen.findByRole('form', { name: 'Restate goal' }));
    await user.clear(form.getByLabelText('Goal'));
    await user.type(form.getByLabelText('Goal'), 'Retire the beacon.');
    await user.click(form.getByRole('button', { name: 'Save goal' }));

    expect(await screen.findByText('Retire the beacon.', { selector: 'p.max-w-prose' })).toBeInTheDocument();
  });

  it('restores the saved goal when the restate dialog is reopened after a cancel', async () => {
    const user = userEvent.setup();
    renderApp('/projects/proj-beacon');

    await user.click(await screen.findByRole('button', { name: 'Restate goal' }));
    let form = within(await screen.findByRole('form', { name: 'Restate goal' }));
    const original = (form.getByLabelText('Goal') as HTMLTextAreaElement).value;
    await user.type(form.getByLabelText('Goal'), ' discarded');
    await user.click(form.getByRole('button', { name: 'Cancel' }));
    await waitFor(() => expect(screen.queryByRole('form', { name: 'Restate goal' })).not.toBeInTheDocument());

    await user.click(screen.getByRole('button', { name: 'Restate goal' }));
    form = within(await screen.findByRole('form', { name: 'Restate goal' }));
    expect(form.getByLabelText('Goal')).toHaveValue(original);
  });

  it('archives after confirmation and then offers unarchive', async () => {
    const user = userEvent.setup();
    const { projects } = renderApp('/projects/proj-beacon');

    await user.click(await screen.findByRole('button', { name: 'Archive' }));
    const dialog = await screen.findByRole('alertdialog');
    expect((await projects.get('proj-beacon'))?.revision).toBe(1);
    await user.click(within(dialog).getByRole('button', { name: 'Archive project' }));

    expect(await screen.findByRole('button', { name: 'Unarchive' })).toBeEnabled();
    expect(screen.getByRole('button', { name: 'Rename' })).toBeDisabled();

    await user.click(screen.getByRole('button', { name: 'Unarchive' }));
    await user.click(within(await screen.findByRole('alertdialog')).getByRole('button', { name: 'Unarchive project' }));

    expect(await screen.findByRole('button', { name: 'Archive' })).toBeEnabled();
    expect(screen.getByRole('button', { name: 'Rename' })).toBeEnabled();
  });

  it('does not archive when the confirmation is cancelled', async () => {
    const user = userEvent.setup();
    const { projects } = renderApp('/projects/proj-beacon');

    await user.click(await screen.findByRole('button', { name: 'Archive' }));
    await user.click(within(await screen.findByRole('alertdialog')).getByRole('button', { name: 'Cancel' }));

    await waitFor(() => expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument());
    expect((await projects.get('proj-beacon'))?.revision).toBe(1);
  });
});
