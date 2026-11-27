import type { LeaseGranted, SandboxImage, WireSystemTime } from '@trogonai/sandbox-ws-client';
import { wireSystemTimeToDate } from '@trogonai/sandbox-ws-client';

export function printJson(value: unknown): void {
  process.stdout.write(`${JSON.stringify(value, null, 2)}\n`);
}

export function printLeaseGranted(granted: LeaseGranted): void {
  const deadline = wireSystemTimeToDate(granted.deadline);
  process.stdout.write(`lease: ${granted.lease_id}\n`);
  process.stdout.write(`provider: ${granted.provider}\n`);
  process.stdout.write(`deadline: ${deadline.toISOString()}\n`);
}

export function summarizeImage(image: SandboxImage): string {
  switch (image.kind) {
    case 'registry': {
      const ref = image.reference;
      const tagOrDigest = ref.digest ? `@${ref.digest}` : ref.tag ? `:${ref.tag}` : '';
      return `${ref.registry}/${ref.repository}${tagOrDigest}`;
    }
    case 'snapshot':
      return `snapshot:${image.reference.digest}`;
    case 'snapshot_tag':
      return `tag:${image.tag.name}`;
    case 'hyperlight_module':
      return `hyperlight:${image.reference.digest}`;
    case 'native_snapshot':
      return `native:${image.reference.provider}:${image.reference.id}`;
    default:
      return JSON.stringify(image);
  }
}

export function relativeTimeFromUnixSeconds(unixSeconds: number): string {
  const deltaMs = Date.now() - unixSeconds * 1000;
  const deltaSeconds = Math.round(deltaMs / 1000);
  const abs = Math.abs(deltaSeconds);
  const suffix = deltaSeconds >= 0 ? 'ago' : 'from now';

  if (abs < 60) {
    return `${abs}s ${suffix}`;
  }
  if (abs < 3600) {
    return `${Math.round(abs / 60)}m ${suffix}`;
  }
  if (abs < 86400) {
    return `${Math.round(abs / 3600)}h ${suffix}`;
  }
  return `${Math.round(abs / 86400)}d ${suffix}`;
}

export function isoFromUnixSeconds(unixSeconds: number): string {
  return new Date(unixSeconds * 1000).toISOString();
}

export function isoFromWireSystemTime(t: WireSystemTime): string {
  return wireSystemTimeToDate(t).toISOString();
}

export function printTable(headers: string[], rows: string[][]): void {
  if (rows.length === 0) {
    process.stdout.write(`${headers.join('  ')}\n(no rows)\n`);
    return;
  }
  const widths = headers.map((header, i) => Math.max(header.length, ...rows.map((row) => (row[i] ?? '').length)));
  const formatRow = (cells: string[]): string =>
    cells
      .map((cell, i) => cell.padEnd(widths[i]))
      .join('  ')
      .trimEnd();
  process.stdout.write(`${formatRow(headers)}\n`);
  for (const row of rows) {
    process.stdout.write(`${formatRow(row)}\n`);
  }
}
