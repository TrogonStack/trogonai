import { CheckIcon, CopyIcon } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';

import { Button } from '@/components/ui/button';
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip';

export function CopyButton({ value, label }: { value: string; label: string }) {
  const [copied, setCopied] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);

  useEffect(() => () => clearTimeout(timer.current), []);

  const copy = async () => {
    try {
      await navigator.clipboard?.writeText(value);
    } catch {
      return;
    }
    setCopied(true);
    clearTimeout(timer.current);
    timer.current = setTimeout(() => setCopied(false), 1400);
  };

  return (
    <Tooltip>
      <TooltipTrigger
        render={
          <Button
            type="button"
            variant="ghost"
            size="icon-xs"
            aria-label={copied ? `${label} copied` : `Copy ${label}`}
            className="text-muted-foreground relative overflow-visible"
            onClick={() => void copy()}
          />
        }
      >
        {copied ? <CheckIcon className="text-success-ink" /> : <CopyIcon />}
        {copied ? (
          <span
            aria-hidden="true"
            className="micro bg-success-soft text-success-ink pointer-events-none absolute top-1/2 left-[calc(100%+4px)] -translate-y-1/2 rounded-sm px-1 text-[9px]"
          >
            Copied
          </span>
        ) : null}
      </TooltipTrigger>
      <TooltipContent>{copied ? 'Copied' : `Copy ${label}`}</TooltipContent>
    </Tooltip>
  );
}

export function truncateMiddle(value: string, head = 8, tail = 6): string {
  return value.length <= head + tail + 1 ? value : `${value.slice(0, head)}…${value.slice(-tail)}`;
}
