import { CopyButton, truncateMiddle } from '@/components/copy-button';
import { cn } from '@/lib/utils';

export function Digest({ hex, label, className }: { hex: string; label: string; className?: string }) {
  return (
    <span className={cn('inline-flex min-w-0 items-center gap-1 font-mono text-xs', className)}>
      <span title={`sha256:${hex}`} className="truncate">
        <span className="text-muted-foreground">sha256:</span>
        {truncateMiddle(hex, 10, 8)}
      </span>
      <CopyButton value={hex} label={label} />
    </span>
  );
}
