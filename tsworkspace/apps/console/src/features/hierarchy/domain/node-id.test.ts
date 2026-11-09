import { describe, expect, it } from 'vitest';

import { HierarchyFault } from './faults';
import { mintNodeId, newNodeIdProblem, parseNewNodeId, parseNodeId, suggestNodeId } from './node-id';

describe('node id', () => {
  it('refuses the id the create route already uses', () => {
    expect(suggestNodeId('New')).toBe('new');
    expect(newNodeIdProblem('new')).toBeDefined();
    expect(parseNewNodeId('new')).toBeUndefined();
    expect(parseNodeId('new')).toBeUndefined();
    expect(() => mintNodeId('new')).toThrow(HierarchyFault);
  });

  it('still accepts ids that only contain the reserved word', () => {
    expect(parseNewNodeId('new-york')).toBe('new-york');
  });
});
