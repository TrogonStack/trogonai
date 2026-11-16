import { fireEvent, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';

import { renderApp } from '@/test/render-app';

const NEW_ARN = 'arn:aws:kms:eu-west-1:512049177301:key/1c0e7d52-9b4a-4f6e-8a31-6d2f90b7c418';

function log(label: string, ms: number, extra = '') {
  console.log(`[probe timing] ${label} ${Math.round(ms)}ms ${extra}`);
}

async function time<T>(label: string, fn: () => Promise<T> | T, extra = ''): Promise<T> {
  const start = performance.now();
  const result = await fn();
  log(label, performance.now() - start, extra);
  return result;
}

async function openKeyStep() {
  const user = userEvent.setup();
  renderApp('/keys/keys/new?backend=kbe_aws_acme_prod');
  await user.click(await screen.findByRole('button', { name: 'Continue' }));
  return screen.getByLabelText(/key locator/) as HTMLInputElement;
}

describe('wizard timing probe', () => {
  it('measures bare event loop yields', async () => {
    await time('200 setTimeout(0) yields', async () => {
      for (let i = 0; i < 200; i++) await new Promise<void>((resolve) => setTimeout(resolve, 0));
    });
    await time('200 microtask yields', async () => {
      for (let i = 0; i < 200; i++) await Promise.resolve();
    });
  });

  it('measures typing the key locator', async () => {
    const input = await time('open key step', () => openKeyStep());
    log('dom size', 0, `elements=${document.querySelectorAll('*').length}`);
    const typing = userEvent.setup();
    const instant = userEvent.setup({ delay: null });
    for (let round = 0; round < 2; round++) {
      await time(`round ${round} type delay=0`, () => typing.type(input, NEW_ARN), `chars=${NEW_ARN.length}`);
      await time(`round ${round} clear`, () => typing.clear(input));
      await time(`round ${round} type delay=null`, () => instant.type(input, NEW_ARN), `chars=${NEW_ARN.length}`);
      await time(`round ${round} clear`, () => instant.clear(input));
      await time(
        `round ${round} fireEvent.change per char`,
        () => {
          for (let i = 1; i <= NEW_ARN.length; i++) fireEvent.change(input, { target: { value: NEW_ARN.slice(0, i) } });
        },
        `chars=${NEW_ARN.length}`,
      );
      fireEvent.change(input, { target: { value: '' } });
    }
    expect(input.value).toBe('');
  });

  it('measures queries on the key step', async () => {
    await openKeyStep();
    for (let round = 0; round < 3; round++) {
      await time(`round ${round} getByRole radio by name`, () =>
        screen.getByRole('radio', { name: /Session artifacts/ }),
      );
      await time(`round ${round} queryByRole radio regex`, () =>
        screen.queryByRole('radio', { name: /credential|secret/i }),
      );
      await time(`round ${round} getByRole button by name`, () =>
        screen.getByRole('button', { name: 'Continue to validation' }),
      );
      await time(`round ${round} getByLabelText`, () => screen.getByLabelText(/key locator/));
      await time(`round ${round} getByText`, () => screen.getByText('Capabilities'));
      await time(`round ${round} click radio`, () => userEvent.setup().click(screen.getByRole('radio', { name: /Session messages/ })));
    }
  });

  it('prints the probe output', () => {
    throw new Error('Timing probe: fail on purpose so CI prints this target log.');
  });
});
