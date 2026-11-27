import assert from 'node:assert/strict';
import { test } from 'node:test';

import { chatModel } from './model.ts';

for (const value of [undefined, '', '   ']) {
  test(`an unset model setting ${JSON.stringify(value)} is rejected instead of picking a default`, () => {
    assert.throws(() => chatModel({ SANDBOX_EXAMPLE_MODEL: value }), /SANDBOX_EXAMPLE_MODEL/);
  });
}

test('the scripted model needs no endpoint or key', () => {
  const model = chatModel({ SANDBOX_EXAMPLE_MODEL: 'scripted' });
  assert.ok(typeof model === 'object');
  assert.equal(model.modelId, 'scripted');
});

test('a configured model without an endpoint is rejected', () => {
  assert.throws(() => chatModel({ SANDBOX_EXAMPLE_MODEL: 'example-model' }), /SANDBOX_EXAMPLE_MODEL_BASE_URL/);
});

test('a configured model uses the id and endpoint from the environment', () => {
  const model = chatModel({
    SANDBOX_EXAMPLE_MODEL: ' example-model ',
    SANDBOX_EXAMPLE_MODEL_BASE_URL: 'http://models.invalid/v1',
    SANDBOX_EXAMPLE_MODEL_API_KEY: 'test-only-placeholder',
  });
  assert.ok(typeof model === 'object');
  assert.equal(model.modelId, 'example-model');
});
