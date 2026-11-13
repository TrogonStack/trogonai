#!/usr/bin/env node
// Downloads node's lib/path.js for each release the connector vetting probes record, and checks each against its
// reviewed digest, so the probes always run the exact path code those releases ship.
//
// usage: node fetch_node_path.mjs DESTINATION
import { createHash } from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import { NODE_PATH_DIGESTS } from './node_releases.mjs';

const destination = process.argv[2];
if (!destination || process.argv.length !== 3) {
  process.stderr.write('usage: fetch_node_path.mjs DESTINATION\n');
  process.exit(2);
}
fs.mkdirSync(destination, { recursive: true });
for (const [release, digest] of Object.entries(NODE_PATH_DIGESTS)) {
  const response = await fetch(`https://raw.githubusercontent.com/nodejs/node/v${release}/lib/path.js`);
  if (!response.ok) throw new Error(`node ${release} lib/path.js: HTTP ${response.status}`);
  const body = Buffer.from(await response.arrayBuffer());
  const actual = createHash('sha256').update(body).digest('hex');
  if (actual !== digest) throw new Error(`node ${release} lib/path.js does not match the reviewed digest`);
  fs.writeFileSync(path.join(destination, `path-${release}.js`), body);
}
