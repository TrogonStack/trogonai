'use client';

import { useChat } from '@ai-sdk/react';
import { DefaultChatTransport } from 'ai';
import { useEffect, useState } from 'react';

import { previewStateSchema } from '../contracts.ts';
import type { PreviewState } from './api/preview/route.ts';

const pane = { flex: 1, display: 'flex', flexDirection: 'column', minWidth: 0, height: '100vh' } as const;

export default function Page() {
  const { messages, sendMessage, status } = useChat({ transport: new DefaultChatTransport({ api: '/api/chat' }) });
  const [input, setInput] = useState('');
  const [preview, setPreview] = useState<PreviewState>({ status: 'not_ready' });
  const busy = status === 'submitted' || status === 'streaming';

  // Ask for the preview URL when the page loads and after each finished agent turn.
  useEffect(() => {
    if (busy) return;
    fetch('/api/preview')
      .then(async (r) => previewStateSchema.parse(await r.json()))
      .then(setPreview)
      .catch(() => {});
  }, [busy]);

  return (
    <main style={{ display: 'flex' }}>
      <section style={{ ...pane, borderRight: '1px solid #ddd' }}>
        <div style={{ flex: 1, overflow: 'auto', padding: 16 }}>
          {messages.map((message) => (
            <div key={message.id} style={{ marginBottom: 12 }}>
              <strong>{message.role}</strong>
              {message.parts.map((part, i) =>
                part.type === 'text' ? (
                  <p key={i} style={{ margin: '4px 0' }}>
                    {part.text}
                  </p>
                ) : part.type.startsWith('tool-') ? (
                  <details key={i} style={{ margin: '4px 0', fontSize: 13 }}>
                    <summary>
                      {part.type.slice('tool-'.length)} ({'state' in part ? part.state : ''})
                    </summary>
                    <pre style={{ whiteSpace: 'pre-wrap' }}>
                      {JSON.stringify('input' in part ? part.input : undefined, null, 2)}
                      {'\n'}
                      {'output' in part && part.output !== undefined ? String(part.output) : ''}
                    </pre>
                  </details>
                ) : null,
              )}
            </div>
          ))}
        </div>
        <form
          style={{ display: 'flex', gap: 8, padding: 16, borderTop: '1px solid #ddd' }}
          onSubmit={(e) => {
            e.preventDefault();
            if (input.trim() === '' || busy) return;
            void sendMessage({ text: input });
            setInput('');
          }}
        >
          <input
            style={{ flex: 1, padding: 8 }}
            value={input}
            onChange={(e) => setInput(e.target.value)}
            placeholder="Describe the app you want"
          />
          <button type="submit" disabled={busy}>
            {busy ? 'Working' : 'Send'}
          </button>
        </form>
      </section>
      <section style={pane}>
        {preview.status === 'ready' ? (
          <iframe src={preview.url} title="Live preview" style={{ flex: 1, border: 0 }} />
        ) : (
          <p style={{ padding: 16, color: '#666' }}>
            {preview.status === 'unavailable' ? preview.reason : 'No app is running yet. Ask the agent to build one.'}
          </p>
        )}
      </section>
    </main>
  );
}
