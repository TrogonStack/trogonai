import { Link } from '@tanstack/react-router';

import { cn } from '@/lib/utils';

import type { KeyRef } from '../domain/identifiers';

export function KeyRefLink({ keyRef, className }: { keyRef: KeyRef; className?: string }) {
  return (
    <Link
      to="/keys/keys/$keyRef"
      params={{ keyRef }}
      className={cn('font-mono text-xs outline-none hover:underline focus-visible:underline', className)}
    >
      {keyRef}
    </Link>
  );
}
