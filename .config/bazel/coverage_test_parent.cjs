const { spawnSync } = require('node:child_process');

function runParent() {
  return 'parent executed';
}

console.log(JSON.stringify({ subject: runParent(), coverage: Boolean(process.env.NODE_V8_COVERAGE) }));
const child = spawnSync(process.execPath, [process.argv[2]], { stdio: 'inherit' });
if (child.error) throw child.error;
process.exitCode = child.status ?? 1;
