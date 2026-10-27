import { describe, expect, it } from 'vitest';

import { ProjectFault } from './faults';
import { MAX_PROJECT_ID_LENGTH, mintProjectId, parseProjectId, projectIdProblem, suggestProjectId } from './project-id';

describe('project id', () => {
  it('accepts lowercase letters, digits and hyphens that start with a letter', () => {
    expect(parseProjectId('customer-portal-2')).toBe('customer-portal-2');
  });

  it.each(['', 'Portal', '1portal', 'my portal', 'my_portal', 'x'.repeat(MAX_PROJECT_ID_LENGTH + 1)])(
    'rejects %j',
    (value) => {
      expect(projectIdProblem(value)).toBeDefined();
      expect(parseProjectId(value)).toBeUndefined();
    },
  );

  it('throws a typed fault when minting an invalid id', () => {
    expect(() => mintProjectId('Nope')).toThrow(ProjectFault);
  });

  it('suggests an id from a name', () => {
    expect(suggestProjectId('Customer Portal!')).toBe('customer-portal');
    expect(suggestProjectId('  ')).toBe('');
  });
});
