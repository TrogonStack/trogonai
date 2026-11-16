import { describe, expect, it } from 'vitest';

describe('declared metacharacter control', () => {
  it('is collected under its literal declared name', () => {
    console.log('[control] declared[12].test.tsx collected');
    expect(true).toBe(true);
  });

  it('prints the control output', () => {
    throw new Error('Control: fail on purpose so CI prints this target log.');
  });
});
