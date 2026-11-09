import { describe, expect, it } from 'vitest';

import { evaluateSnippet, extractBlock } from './loader.ts';

const SOURCE = `class Example {
  keep() { return 1; }
  async target(argument: string): Promise<number> {
    if (argument) { return { nested: true }.nested ? 1 : 2; }
    return 0;
  }
  other() { return 3; }
}`;

describe('extractBlock', () => {
  it('returns the method with balanced braces', () => {
    const block = extractBlock(SOURCE, /async target\(/, /Promise<number> \{/);
    expect(block.startsWith('async target(')).toBe(true);
    expect(block.endsWith('}')).toBe(true);
    expect(block).not.toContain('other()');
  });

  it('fails when the start is missing', () => {
    expect(() => extractBlock(SOURCE, /missing\(/, /\{/)).toThrow();
  });
});

describe('evaluateSnippet', () => {
  it('runs transpiled code against the given scope', () => {
    expect(evaluateSnippet('module.exports = offset + 1;', { offset: 41 })).toBe(42);
  });
});
