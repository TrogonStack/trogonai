const fs = require('node:fs');
const request = JSON.parse(process.argv[1]);
const readKernel = (pid) => {
  try {
    const value = fs.readFileSync(`/proc/${pid}/stat`, 'utf8');
    const fields = value
      .slice(value.lastIndexOf(')') + 1)
      .trim()
      .split(/\s+/);
    return { pid: Number(pid), startTime: fields[19], ppid: Number(fields[1]) };
  } catch (error) {
    if (error.code === 'ENOENT' || error.code === 'ESRCH') return undefined;
    throw error;
  }
};
const readStat = (pid) => {
  const value = readKernel(pid);
  return value && { pid: value.pid, startTime: value.startTime };
};
const readRecord = (path, limit) => {
  const descriptor = fs.openSync(path, fs.constants.O_RDONLY | fs.constants.O_NOFOLLOW | fs.constants.O_NONBLOCK);
  try {
    const stat = fs.fstatSync(descriptor);
    if (!stat.isFile() || stat.size > limit) throw Error('native_record_bound_exceeded');
    const bytes = Buffer.alloc(limit + 1);
    let length = 0;
    while (length < bytes.length) {
      const count = fs.readSync(descriptor, bytes, length, bytes.length - length, null);
      if (count === 0) break;
      length += count;
    }
    if (length > limit) throw Error('native_record_bound_exceeded');
    return bytes.subarray(0, length).toString();
  } finally {
    fs.closeSync(descriptor);
  }
};
const paths = request.paths.map((path) => `/proc/1/root${path}`);
const ready = paths.slice(0, 2).every((path) => fs.existsSync(path));
const records = ready ? paths.slice(0, 2).map((path) => JSON.parse(readRecord(path, 65536))) : [];
const pids = fs.readdirSync('/proc').filter((value) => /^\d+$/.test(value));
if (pids.length > 4096) throw Error('native_process_census_exceeded');
const members = [];
for (const pid of pids) {
  let environment;
  try {
    environment = fs.readFileSync(`/proc/${pid}/environ`).toString().split('\0');
  } catch (error) {
    if (error.code === 'ENOENT' || error.code === 'ESRCH') continue;
    throw error;
  }
  if (environment.includes(`SANDBOX_ACCEPTANCE_FAMILY=${request.marker}`)) {
    const identity = readStat(pid);
    if (identity) members.push(identity);
  }
}
const pinned = new Map(
  [...records, ...request.tracked].map((identity) => [`${identity.pid}:${identity.startTime}`, identity]),
);
const tracked = [...pinned.values()].map((identity) => {
  const observed = readStat(identity.pid);
  return {
    pid: identity.pid,
    startTime: identity.startTime,
    state: observed && observed.startTime === identity.startTime ? 'alive' : 'absent',
  };
});
const starts = fs.existsSync(paths[2]) ? readRecord(paths[2], 1048576).trim().split('\n').filter(Boolean) : [];
const status = fs.readFileSync('/proc/self/status', 'utf8');
const caps = BigInt(`0x${/^CapEff:\s*([a-f0-9]+)$/m.exec(status)[1]}`);
const root = records[0] && readKernel(records[0].pid);
const child = records[1] && readKernel(records[1].pid);
const rootChildLinked = Boolean(
  root &&
  child &&
  root.startTime === records[0].startTime &&
  child.startTime === records[1].startTime &&
  child.ppid === root.pid,
);
process.stdout.write(
  JSON.stringify({
    native: { ready, records, members, tracked, starts },
    proof: {
      observer: readStat(process.pid),
      init: readStat(1),
      uid: process.getuid(),
      ptrace: (caps & (1n << 19n)) !== 0n,
      rootChildLinked,
    },
  }),
);
