import { spawnSync } from 'node:child_process';
import { existsSync } from 'node:fs';

const flags = process.allowedNodeEnvironmentFlags.has('--experimental-transform-types')
  ? ['--experimental-transform-types']
  : [];
const args = process.argv.slice(2).flatMap((arg) => {
  const prefix = '--env-file-if-exists=';
  if (!arg.startsWith(prefix)) return [arg];
  const file = arg.slice(prefix.length);
  return existsSync(file) ? ['--env-file=' + file] : [];
});
const child = spawnSync(process.execPath, [...flags, ...args], { stdio: 'inherit' });
if (child.error !== undefined) throw child.error;
if (child.signal !== null) process.kill(process.pid, child.signal);
else process.exitCode = child.status ?? 1;
