import type { LucideIcon } from 'lucide-react';
import type { ReactNode } from 'react';

import { CountUp } from '@/components/count-up';
import { RegistrationMarks, SpotlightSurface } from '@/components/spotlight-card';
import { Skeleton } from '@/components/ui/skeleton';
import { cn } from '@/lib/utils';

export interface StatProps {
  label: string;
  value: number;
  caption: string;
  icon: LucideIcon;
  led: string;
  blink?: boolean;
}

export function Stat({ label, value, caption, icon: Icon, led, blink }: StatProps) {
  return (
    <SpotlightSurface className="flex flex-col gap-4 p-4">
      <RegistrationMarks />
      <div className="flex items-center justify-between gap-2">
        <span className="micro text-muted-foreground flex items-center gap-2">
          <span aria-hidden="true" className={cn('led size-1.5 rounded-full', led, blink && 'led-blink')} />
          {label}
        </span>
        <Icon className="text-muted-foreground/70 size-3.5" aria-hidden="true" />
      </div>
      <div className="flex flex-col gap-1">
        <span className="font-mono text-4xl leading-10 font-semibold tracking-tight tabular-nums">
          <CountUp value={value} />
        </span>
        <span className="micro text-muted-foreground text-[10px]">{caption}</span>
      </div>
    </SpotlightSurface>
  );
}

export function StatStrip({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div aria-label={label} role="group" className="grid grid-cols-2 gap-3 xl:grid-cols-4">
      {children}
    </div>
  );
}

export function StatStripSkeleton() {
  return (
    <div className="grid grid-cols-2 gap-3 xl:grid-cols-4">
      {Array.from({ length: 4 }, (_, index) => (
        <div key={index} className="border-border bg-card flex flex-col gap-4 rounded-lg border p-4">
          <Skeleton className="h-3 w-24" />
          <div className="flex flex-col gap-2">
            <Skeleton className="h-9 w-14" />
            <Skeleton className="h-3 w-28" />
          </div>
        </div>
      ))}
    </div>
  );
}
