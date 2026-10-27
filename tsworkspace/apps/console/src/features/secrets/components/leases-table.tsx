import { truncateMiddle } from '@/components/copy-button';
import { StatusPill, type Tone } from '@/components/lifecycle-badge';
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table';

import type { LeaseRecord } from '../domain/credential-state';
import { leaseEndLabels } from '../domain/labels';
import { RelativeTime } from './relative-time';
import { countdown, useNow } from './time';

const FIVE_MINUTES = 5 * 60 * 1000;

function leaseStatus(lease: LeaseRecord, now: Date): { label: string; tone: Tone; blink?: boolean } {
  if (lease.endedAt) {
    return {
      label: lease.endReason === undefined ? 'Ended' : `Ended, ${leaseEndLabels[lease.endReason].toLowerCase()}`,
      tone: 'neutral',
    };
  }
  if (!lease.issuedAt) return { label: 'Reserved', tone: 'warning', blink: true };
  if (lease.expiresAt && lease.expiresAt <= now) return { label: 'Expired locally', tone: 'warning' };
  return { label: 'Live', tone: 'success', blink: true };
}

export function LeasesTable({ leases }: { leases: readonly LeaseRecord[] }) {
  const now = useNow();
  if (leases.length === 0) {
    return (
      <p className="text-muted-foreground text-[13px]">
        No lease has been issued. Only provider, tool and MCP authentication secrets are leased, for at most five
        minutes.
      </p>
    );
  }
  return (
    <div className="border-border bg-card overflow-hidden rounded-lg border">
      <Table aria-label="Leases">
        <TableHeader>
          <TableRow className="hover:bg-transparent">
            <TableHead>Lease</TableHead>
            <TableHead>Workload</TableHead>
            <TableHead>Audience</TableHead>
            <TableHead>Issued</TableHead>
            <TableHead>Remaining</TableHead>
            <TableHead>Status</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {leases.map((lease) => {
            const status = leaseStatus(lease, now);
            const remaining = lease.expiresAt ? lease.expiresAt.getTime() - now.getTime() : undefined;
            const showCountdown =
              !lease.endedAt && lease.expiresAt !== undefined && remaining !== undefined && remaining > 0;
            return (
              <TableRow key={lease.id}>
                <TableCell className="text-muted-foreground font-mono text-xs" title={lease.id}>
                  {truncateMiddle(lease.id, 8, 6)}
                </TableCell>
                <TableCell className="font-mono text-xs">{lease.workloadPrincipalId}</TableCell>
                <TableCell className="text-muted-foreground text-[13px]">{lease.audience}</TableCell>
                <TableCell className="text-muted-foreground text-xs">
                  {lease.issuedAt ? <RelativeTime date={lease.issuedAt} /> : '-'}
                </TableCell>
                <TableCell className="font-mono text-xs tabular-nums">
                  {showCountdown && lease.expiresAt ? (
                    <span
                      role="timer"
                      aria-label="Lease remaining"
                      className={(remaining ?? 0) <= FIVE_MINUTES ? 'text-warning-ink' : undefined}
                    >
                      {countdown(lease.expiresAt, now)}
                    </span>
                  ) : (
                    '-'
                  )}
                </TableCell>
                <TableCell>
                  <StatusPill tone={status.tone} blink={status.blink} label={status.label} />
                </TableCell>
              </TableRow>
            );
          })}
        </TableBody>
      </Table>
    </div>
  );
}
