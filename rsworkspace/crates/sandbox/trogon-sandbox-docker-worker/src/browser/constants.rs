use std::time::Duration;

pub(super) const PROBE_TIMEOUT: Duration = Duration::from_secs(20);
pub(super) const CLEANUP_TIMEOUT: Duration = Duration::from_secs(5);
pub(super) const BROWSER_USER: &str = "65532:65532";
pub(super) const BROWSER_HOME: &str = "/tmp";
pub(super) const MAX_PIDS: i64 = 512;
pub(super) const MAX_SHM_MIB: u64 = 512;

pub(super) const OUTPUT_LIMIT: usize = 64 * 1024;

pub(super) const PROBE_SCRIPT: &str = r#"
import { access, mkdtemp, readFile, rm } from 'node:fs/promises';
import { spawnSync } from 'node:child_process';
const directory = await mkdtemp('/tmp/sandbox-browser-probe-');
let png;
try {
  const result = spawnSync('chromium', [
    '--headless=new', '--disable-gpu', '--no-sandbox', '--no-first-run',
    '--disable-background-networking', '--disable-component-update', '--disable-sync',
    '--user-data-dir=' + directory + '/profile', '--screenshot=' + directory + '/page.png',
    '--window-size=160,120', '--timeout=10000',
    'data:text/html,<title>Sandbox browser probe</title><h1>ready</h1>'
  ], { encoding: 'utf8', timeout: 15000, maxBuffer: 16384 });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error('Chromium failed: ' + result.stderr.slice(-4096));
  png = (await readFile(directory + '/page.png')).toString('base64');
} finally { await rm(directory, { recursive: true, force: true }); }
const temporary_removed = await access(directory).then(() => false, error => {
  if (error.code === 'ENOENT') return true;
  throw error;
});
process.stdout.write(JSON.stringify({ node: process.versions.node, uid: process.getuid(), home: process.env.HOME, png, temporary_removed }));
"#;
