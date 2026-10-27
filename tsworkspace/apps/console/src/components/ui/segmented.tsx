import { Toggle } from '@base-ui/react/toggle';
import { ToggleGroup } from '@base-ui/react/toggle-group';
import type { ReactNode } from 'react';

import { cn } from '@/lib/utils';

export interface SegmentedOption<T extends string> {
  readonly value: T;
  readonly label: string;
  readonly count?: number;
  readonly icon?: ReactNode;
}

interface SegmentedProps<T extends string> {
  label: string;
  value: T;
  options: readonly SegmentedOption<T>[];
  onChange: (value: T) => void;
  size?: 'sm' | 'md';
  iconOnly?: boolean;
  className?: string;
  disabled?: boolean;
}

export function Segmented<T extends string>({
  label,
  value,
  options,
  onChange,
  size = 'md',
  iconOnly,
  className,
  disabled,
}: SegmentedProps<T>) {
  return (
    <ToggleGroup
      aria-label={label}
      value={[value]}
      disabled={disabled}
      onValueChange={(next) => {
        const selected = next[0];
        if (selected !== undefined) onChange(selected as T);
      }}
      className={cn('well inline-flex items-center gap-0.5 rounded-md border border-border p-0.5', className)}
    >
      {options.map((option) => (
        <Toggle
          key={option.value}
          value={option.value}
          aria-label={iconOnly ? option.label : undefined}
          title={iconOnly ? option.label : undefined}
          className={cn(
            'group/seg inline-flex items-center justify-center gap-1.5 rounded-[5px] border border-transparent font-mono font-medium tracking-wide whitespace-nowrap text-muted-foreground uppercase transition-[background-color,box-shadow,color,transform] duration-100 outline-none select-none hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/70 disabled:pointer-events-none disabled:opacity-50 data-pressed:border-border data-pressed:bg-[var(--key-face)] data-pressed:text-foreground data-pressed:shadow-[0_1px_0_0_var(--key-edge),inset_0_1px_0_0_var(--highlight)] [&_svg]:size-3.5 [&_svg]:shrink-0',
            size === 'sm' ? 'h-6 px-2 text-[10.5px]' : 'h-7 px-2.5 text-[11px]',
            iconOnly && 'px-1.5',
          )}
        >
          {iconOnly ? null : (
            <span
              aria-hidden="true"
              className="bg-muted-foreground/30 group-data-pressed/seg:bg-primary size-1 shrink-0 rounded-full transition-[background-color,box-shadow] group-data-pressed/seg:shadow-[0_0_6px_1px_var(--primary)]"
            />
          )}
          {option.icon}
          {iconOnly ? null : option.label}
          {option.count === undefined ? null : (
            <span className="text-muted-foreground text-[10.5px] tabular-nums">{option.count}</span>
          )}
        </Toggle>
      ))}
    </ToggleGroup>
  );
}
