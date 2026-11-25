import type { CSSProperties } from 'react';
import { DayPicker, type DayPickerProps } from 'react-day-picker';

import 'react-day-picker/style.css';

import { cn } from '@/lib/utils';

export function Calendar({ className, style, ...props }: DayPickerProps) {
  return (
    <DayPicker
      timeZone="UTC"
      showOutsideDays
      className={cn(
        'p-3 text-[13px] [&_button]:outline-none [&_button]:focus-visible:ring-2 [&_button]:focus-visible:ring-ring',
        className,
      )}
      style={
        {
          '--rdp-accent-color': 'var(--primary-ink)',
          '--rdp-accent-background-color': 'var(--primary)',
          '--rdp-day-height': '34px',
          '--rdp-day-width': '34px',
          '--rdp-day_button-height': '32px',
          '--rdp-day_button-width': '32px',
          '--rdp-day_button-border-radius': '6px',
          '--rdp-nav-height': '32px',
          '--rdp-nav_button-height': '28px',
          '--rdp-nav_button-width': '28px',
          '--rdp-today-color': 'var(--primary-ink)',
          ...style,
        } as CSSProperties
      }
      {...props}
    />
  );
}
