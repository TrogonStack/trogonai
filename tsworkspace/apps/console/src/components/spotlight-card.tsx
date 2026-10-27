import { useRef, type ComponentProps, type MouseEvent } from 'react';

import { cn } from '@/lib/utils';

export function SpotlightSurface({ className, children, onMouseMove, ...props }: ComponentProps<'div'>) {
  const ref = useRef<HTMLDivElement>(null);

  const track = (event: MouseEvent<HTMLDivElement>) => {
    const element = ref.current;
    if (element) {
      const rect = element.getBoundingClientRect();
      element.style.setProperty('--mx', `${event.clientX - rect.left}px`);
      element.style.setProperty('--my', `${event.clientY - rect.top}px`);
    }
    onMouseMove?.(event);
  };

  return (
    <div
      ref={ref}
      onMouseMove={track}
      className={cn(
        'group/spot relative isolate overflow-hidden rounded-lg border border-border bg-card shadow-[inset_0_1px_0_0_var(--highlight)]',
        className,
      )}
      {...props}
    >
      <span
        aria-hidden="true"
        className="pointer-events-none absolute inset-0 -z-10 opacity-0 transition-opacity duration-300 group-hover/spot:opacity-100"
        style={{
          background:
            'radial-gradient(260px circle at var(--mx, 50%) var(--my, 50%), var(--spotlight), transparent 70%)',
        }}
      />
      {children}
    </div>
  );
}

export function RegistrationMarks({ className }: { className?: string }) {
  const mark = 'absolute size-2 border-muted-foreground/45';
  return (
    <span aria-hidden="true" className={cn('pointer-events-none absolute inset-0', className)}>
      <span className={cn(mark, 'top-1 left-1 border-t border-l')} />
      <span className={cn(mark, 'top-1 right-1 border-t border-r')} />
      <span className={cn(mark, 'bottom-1 left-1 border-b border-l')} />
      <span className={cn(mark, 'right-1 bottom-1 border-r border-b')} />
    </span>
  );
}
