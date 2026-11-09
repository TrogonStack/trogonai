import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table';

import type { UseRecord } from '../domain/credential-state';
import { failureReasonMessage } from '../domain/faults';
import {
  authorizationEndLabels,
  useActionLabels,
  useDirectionLabels,
  useModeLabels,
  useResultLabels,
} from '../domain/labels';
import { formatTimestamp, relativeTime } from './time';

export function UsesTable({ uses }: { uses: readonly UseRecord[] }) {
  if (uses.length === 0) {
    return (
      <p className="text-muted-foreground text-[13px]">
        No use has been authorized. A workload asks for an authorization each time it needs the secret.
      </p>
    );
  }
  return (
    <div className="border-border bg-card overflow-hidden rounded-lg border">
      <Table aria-label="Use authorizations">
        <TableHeader>
          <TableRow className="hover:bg-transparent">
            <TableHead>Issued</TableHead>
            <TableHead>Workload</TableHead>
            <TableHead>Operation</TableHead>
            <TableHead>Action</TableHead>
            <TableHead>Mode</TableHead>
            <TableHead>Version</TableHead>
            <TableHead>Result</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {uses.map((use) => (
            <TableRow key={use.id}>
              <TableCell>
                <time
                  dateTime={use.issuedAt.toISOString()}
                  title={formatTimestamp(use.issuedAt)}
                  className="text-muted-foreground text-xs"
                >
                  {relativeTime(use.issuedAt)}
                </time>
              </TableCell>
              <TableCell className="font-mono text-xs">{use.workloadPrincipalId}</TableCell>
              <TableCell className="text-muted-foreground font-mono text-xs">{use.operationId}</TableCell>
              <TableCell className="text-[13px]">
                {useActionLabels[use.action]}
                <span className="text-muted-foreground"> / {useDirectionLabels[use.direction]}</span>
              </TableCell>
              <TableCell className="text-muted-foreground text-[13px]">{useModeLabels[use.mode]}</TableCell>
              <TableCell className="font-mono text-xs tabular-nums">
                {use.version === undefined ? '-' : `v${use.version}`}
              </TableCell>
              <TableCell className="text-[13px]">
                {use.result === undefined ? (
                  <span className="text-muted-foreground">Open</span>
                ) : (
                  <span title={use.failureReason === undefined ? undefined : failureReasonMessage(use.failureReason)}>
                    {useResultLabels[use.result]}
                  </span>
                )}
                {use.endedReason === undefined ? null : (
                  <span className="text-muted-foreground"> / {authorizationEndLabels[use.endedReason]}</span>
                )}
              </TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
    </div>
  );
}
