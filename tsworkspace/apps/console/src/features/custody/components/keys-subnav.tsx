import { Link } from '@tanstack/react-router';

import { cn } from '@/lib/utils';

const linkClass =
  'text-muted-foreground hover:text-foreground focus-visible:ring-ring/70 data-[status=active]:border-primary data-[status=active]:text-foreground -mb-px border-b-2 border-transparent px-3 py-2 text-[13px] font-medium transition-colors outline-none focus-visible:ring-2';

export function KeysSubnav({ className }: { className?: string }) {
  return (
    <nav aria-label="Encryption keys" className={cn('border-border flex items-center gap-1 border-b', className)}>
      <Link to="/keys" activeOptions={{ exact: true }} className={linkClass}>
        Overview
      </Link>
      <Link to="/keys/backends" className={linkClass}>
        Backends
      </Link>
    </nav>
  );
}
