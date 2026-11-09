import { describe, expect, it } from 'vitest';

import {
  MAX_ENVIRONMENT_ID_LENGTH,
  builtInEnvironmentIds,
  isBuiltInEnvironmentId,
  mintEnvironmentId,
  parseEnvironmentId,
  suggestEnvironmentId,
} from './environment-id';

describe('environment id', () => {
  it.each(['qa', 'load-testing', 'eu-west-2', 'a'])('accepts %s', (value) => {
    expect(parseEnvironmentId(value)).toBe(value);
  });

  it.each(['', 'QA', '1qa', '-qa', 'qa env', 'qa_env', 'café', 'a'.repeat(MAX_ENVIRONMENT_ID_LENGTH + 1)])(
    'rejects %j',
    (value) => {
      expect(parseEnvironmentId(value)).toBeUndefined();
    },
  );

  it('accepts the longest permitted id', () => {
    expect(parseEnvironmentId('a'.repeat(MAX_ENVIRONMENT_ID_LENGTH))).toBeDefined();
  });

  it('refuses to mint an invalid id', () => {
    expect(() => mintEnvironmentId('Not Valid')).toThrow();
  });

  it('knows the built-in ids', () => {
    expect(Object.values(builtInEnvironmentIds)).toEqual(['production', 'preview', 'development']);
    expect(isBuiltInEnvironmentId(builtInEnvironmentIds.preview)).toBe(true);
    expect(isBuiltInEnvironmentId(mintEnvironmentId('qa'))).toBe(false);
  });

  it('suggests an id from a display name', () => {
    expect(suggestEnvironmentId('Load Testing!')).toBe('load-testing');
    expect(suggestEnvironmentId('42 Lab')).toBe('lab');
    expect(parseEnvironmentId(suggestEnvironmentId('Load Testing!'))).toBeDefined();
  });
});
