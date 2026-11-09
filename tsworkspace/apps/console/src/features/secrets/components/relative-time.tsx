import { cn } from '@/lib/utils';

import { formatTimestamp, relativeTime } from './time';

export function RelativeTime({ date, className }: { date: Date; className?: string }) {
  return (
    <time dateTime={date.toISOString()} title={formatTimestamp(date)} className={cn('tabular-nums', className)}>
      {relativeTime(date)}
    </time>
  );
}
