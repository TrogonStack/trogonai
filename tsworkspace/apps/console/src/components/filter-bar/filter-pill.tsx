import { XIcon } from 'lucide-react';
import type { ReactNode } from 'react';

import { cn } from '@/lib/utils';

interface FilterPillProps {
  icon: ReactNode;
  label: string;
  operator: string;
  value: ReactNode;
  onEdit?: () => void;
  onRemove: () => void;
}

const segment = 'flex h-full items-center gap-1.5 px-1.5 whitespace-nowrap';

export function FilterPill({ icon, label, operator, value, onEdit, onRemove }: FilterPillProps) {
  return (
    <span
      data-slot="filter-pill"
      className="border-border bg-surface-deep inline-flex h-6 max-w-full shrink-0 items-stretch overflow-hidden rounded-md border text-xs"
    >
      <span className={cn(segment, 'text-foreground [&_svg]:size-3.5 [&_svg]:text-muted-foreground')}>
        {icon}
        {label}
      </span>
      <span className={cn(segment, 'micro border-l border-border text-[10px] text-muted-foreground')}>{operator}</span>
      {onEdit ? (
        <button
          type="button"
          aria-label={`Edit ${label} filter`}
          onClick={onEdit}
          className={cn(
            segment,
            'min-w-0 cursor-pointer border-l border-border text-foreground outline-none hover:bg-accent focus-visible:bg-accent',
          )}
        >
          {value}
        </button>
      ) : (
        <span className={cn(segment, 'min-w-0 border-l border-border text-foreground')}>{value}</span>
      )}
      <button
        type="button"
        aria-label={`Remove ${label} filter`}
        onClick={onRemove}
        className="border-border text-muted-foreground hover:bg-accent hover:text-foreground focus-visible:bg-accent flex h-full cursor-pointer items-center border-l px-1 outline-none"
      >
        <XIcon aria-hidden="true" className="size-3" />
      </button>
    </span>
  );
}
