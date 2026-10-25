import { describe, expect, it } from 'vitest';

import {
  commandBody,
  ENVELOPE_ID,
  interactionBody,
  interactionPayloadText,
  signedHeaders,
  signSlackRequest,
  socketEnvelope,
} from './wire.ts';

describe('signSlackRequest', () => {
  it('reproduces the signature published in the Slack request verification guide', () => {
    const body =
      'token=xyzz0WbapA4vBCDEFasx0q6G&team_id=T1DC2JH3J&team_domain=testteamnow&channel_id=G8PSS9T3V&channel_name=foobar&user_id=U2CERLKJA&user_name=roadrunner&command=%2Fwebhook-collect&text=&response_url=https%3A%2F%2Fhooks.slack.com%2Fcommands%2FT1DC2JH3J%2F397700885554%2F96rGlfmibIGlgcZRskXaIFfN&trigger_id=398738663015.47445629121.803a0bc887a14d10d2c447fce8b6703c';
    expect(signSlackRequest('8f742231b10e8888abcd99yyyzzz85a5', 1531420618, body)).toBe(
      'v0=a2114d57b48eac39b9ad189dd8316235a7b4a8d21a10bd27519666489c69b503',
    );
  });
});

describe('signedHeaders', () => {
  it('signs the exact body with the fixed timestamp', () => {
    const headers = signedHeaders({ contentType: 'application/json', body: '{}' });
    expect(headers['x-slack-request-timestamp']).toBe('1700000000');
    expect(headers['x-slack-signature']).toBe(signSlackRequest('bolt-conformance-signing-secret', 1_700_000_000, '{}'));
  });
});

describe('form encodings', () => {
  it('wraps an interaction in a single payload field', () => {
    const payload = { type: 'block_actions', text: 'a&b' };
    const form = new URLSearchParams(interactionBody(payload));
    expect([...form.keys()]).toEqual(['payload']);
    expect(form.get('payload')).toBe(interactionPayloadText(payload));
  });

  it('flattens a slash command into form fields', () => {
    const form = new URLSearchParams(commandBody({ command: '/slash', text: 'a&b=c +d' }));
    expect(form.get('command')).toBe('/slash');
    expect(form.get('text')).toBe('a&b=c +d');
  });
});

describe('socketEnvelope', () => {
  it('omits retry metadata unless given', () => {
    const envelope = JSON.parse(socketEnvelope({ type: 'events_api', payload: {}, acceptsResponsePayload: false }));
    expect(envelope).toEqual({
      payload: {},
      envelope_id: ENVELOPE_ID,
      type: 'events_api',
      accepts_response_payload: false,
    });
  });

  it('carries retry metadata', () => {
    const envelope = JSON.parse(
      socketEnvelope({
        type: 'events_api',
        payload: {},
        acceptsResponsePayload: false,
        retryAttempt: 2,
        retryReason: 'timeout',
      }),
    );
    expect(envelope.retry_attempt).toBe(2);
    expect(envelope.retry_reason).toBe('timeout');
  });
});
