import { CheckIcon } from 'lucide-react';

import { cn } from '@/lib/utils';

export function StepIndicator({ steps, current }: { steps: readonly string[]; current: number }) {
  return (
    <ol aria-label="Progress" className="flex flex-wrap items-center gap-x-4 gap-y-2">
      {steps.map((step, index) => {
        const done = index < current;
        const active = index === current;
        return (
          <li key={step} aria-current={active ? 'step' : undefined} className="flex items-center gap-2">
            <span
              className={cn(
                'flex size-5 items-center justify-center rounded-full border font-mono text-[10px]',
                done && 'border-success/40 bg-success-soft text-success-ink',
                active && 'border-primary bg-primary/10 text-primary-ink',
                !done && !active && 'border-border text-muted-foreground',
              )}
            >
              {done ? <CheckIcon className="size-3" aria-hidden="true" /> : index + 1}
            </span>
            <span className={cn('text-[13px]', active ? 'font-medium' : 'text-muted-foreground')}>{step}</span>
            {index < steps.length - 1 ? <span aria-hidden="true" className="bg-border h-px w-6" /> : null}
          </li>
        );
      })}
    </ol>
  );
}
