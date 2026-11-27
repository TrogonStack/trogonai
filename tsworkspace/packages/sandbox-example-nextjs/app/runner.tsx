'use client';

import { useActionState } from 'react';

import type { RunResult } from '../server/run.ts';
import { runScript } from './actions.ts';

export function Runner() {
  const [result, run, pending] = useActionState<RunResult | undefined, FormData>(
    (_previous, form) => runScript(form),
    undefined,
  );
  return (
    <form action={run}>
      <input name="script" defaultValue="echo hello" style={{ width: '100%', padding: 8 }} />
      <button type="submit" disabled={pending} style={{ marginTop: 8 }}>
        {pending ? 'Running' : 'Run'}
      </button>
      {result !== undefined && (
        <pre>
          exit {result.exitCode}
          {'\n'}
          {result.stdout}
          {result.stderr}
        </pre>
      )}
    </form>
  );
}
