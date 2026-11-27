import { Runner } from './runner.tsx';

export default function Page() {
  return (
    <main style={{ maxWidth: 640, margin: '0 auto' }}>
      <h1>Run a script in a sandbox</h1>
      <Runner />
    </main>
  );
}
