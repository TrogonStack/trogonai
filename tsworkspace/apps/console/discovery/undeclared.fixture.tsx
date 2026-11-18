import { expect, it } from 'vitest';

it('is never collected because no target declares it', () => {
  expect.fail('An undeclared test file from the output tree was collected.');
});
