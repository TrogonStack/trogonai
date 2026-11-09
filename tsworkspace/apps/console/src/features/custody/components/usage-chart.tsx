import { USAGE_DAYS, type DailyUsage } from '../domain/key';

function dayLabel(daysAgo: number): string {
  return daysAgo === 0 ? 'Today' : `${daysAgo}d ago`;
}

export function UsageChart({ usage }: { usage: readonly DailyUsage[] }) {
  const peak = Math.max(1, ...usage.flatMap((day) => [day.wraps, day.unwraps]));
  return (
    <div className="flex flex-col gap-2">
      <ol
        aria-label={`Wraps and unwraps per day for the last ${USAGE_DAYS} days, oldest first`}
        className="flex h-32 items-end gap-3"
      >
        {usage.map((day, index) => {
          const daysAgo = usage.length - 1 - index;
          return (
            <li
              key={index}
              title={`${dayLabel(daysAgo)}: ${day.wraps} wraps, ${day.unwraps} unwraps`}
              className="flex h-full min-w-0 flex-1 items-end gap-0.5"
            >
              <span className="bg-primary/70 w-full rounded-t-xs" style={{ height: `${(day.wraps / peak) * 100}%` }} />
              <span className="bg-info/70 w-full rounded-t-xs" style={{ height: `${(day.unwraps / peak) * 100}%` }} />
            </li>
          );
        })}
      </ol>
      <div className="micro text-muted-foreground flex items-center justify-between text-[10px]">
        <span>{usage.length - 1} days ago</span>
        <span className="flex items-center gap-3">
          <span className="flex items-center gap-1.5">
            <span aria-hidden="true" className="bg-primary/70 size-2 rounded-xs" />
            Wraps
          </span>
          <span className="flex items-center gap-1.5">
            <span aria-hidden="true" className="bg-info/70 size-2 rounded-xs" />
            Unwraps
          </span>
        </span>
        <span>Today</span>
      </div>
    </div>
  );
}
