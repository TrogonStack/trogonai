import { cn } from '@/lib/utils';

import type { HourlyDeliveries } from '../domain/delivery';

const SPARK_WIDTH = 96;
const SPARK_HEIGHT = 24;

function totals(hourly: readonly HourlyDeliveries[]): number[] {
  return hourly.map((hour) => hour.accepted + hour.rejected);
}

export function Sparkline({ hourly, className }: { hourly: readonly HourlyDeliveries[]; className?: string }) {
  const values = totals(hourly);
  const peak = Math.max(1, ...values);
  const step = values.length > 1 ? SPARK_WIDTH / (values.length - 1) : 0;
  const points = values
    .map(
      (value, index) =>
        `${(index * step).toFixed(1)},${(SPARK_HEIGHT - 2 - (value / peak) * (SPARK_HEIGHT - 4)).toFixed(1)}`,
    )
    .join(' ');
  const total = values.reduce((sum, value) => sum + value, 0);
  return (
    <svg
      role="img"
      aria-label={`${total} deliveries in the last 24 hours`}
      viewBox={`0 0 ${SPARK_WIDTH} ${SPARK_HEIGHT}`}
      className={cn('text-primary-ink h-6 w-24', className)}
    >
      <polyline points={points} fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinejoin="round" />
    </svg>
  );
}

export function ActivityChart({ hourly }: { hourly: readonly HourlyDeliveries[] }) {
  const peak = Math.max(1, ...totals(hourly));
  return (
    <div className="flex flex-col gap-2">
      <ol aria-label="Deliveries per hour, oldest first" className="flex h-28 items-end gap-1">
        {hourly.map((hour, index) => {
          const hoursAgo = hourly.length - 1 - index;
          const total = hour.accepted + hour.rejected;
          return (
            <li
              key={index}
              title={`${hoursAgo === 0 ? 'This hour' : `${hoursAgo}h ago`}: ${hour.accepted} accepted, ${hour.rejected} rejected`}
              className="flex h-full min-w-0 flex-1 flex-col justify-end"
            >
              <span
                className="flex w-full flex-col justify-end overflow-hidden rounded-xs"
                style={{ height: `${(total / peak) * 100}%` }}
              >
                <span
                  className="bg-danger w-full"
                  style={{ height: `${total === 0 ? 0 : (hour.rejected / total) * 100}%` }}
                />
                <span className="bg-primary/70 w-full flex-1" />
              </span>
            </li>
          );
        })}
      </ol>
      <div className="micro text-muted-foreground flex items-center justify-between text-[10px]">
        <span>24h ago</span>
        <span className="flex items-center gap-3">
          <span className="flex items-center gap-1.5">
            <span aria-hidden="true" className="bg-primary/70 size-2 rounded-xs" />
            Accepted
          </span>
          <span className="flex items-center gap-1.5">
            <span aria-hidden="true" className="bg-danger size-2 rounded-xs" />
            Rejected
          </span>
        </span>
        <span>Now</span>
      </div>
    </div>
  );
}
