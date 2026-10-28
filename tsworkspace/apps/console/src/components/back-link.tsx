import { createLink } from '@tanstack/react-router';
import { ArrowLeftIcon } from 'lucide-react';
import type { ComponentProps, ReactNode } from 'react';

import { buttonVariants } from '@/components/ui/button';
import { cn } from '@/lib/utils';

interface BackAnchorProps extends ComponentProps<'a'> {
  icon?: ReactNode;
}

function BackAnchor({ icon, className, children, ...props }: BackAnchorProps) {
  return (
    <a {...props} className={cn(buttonVariants({ variant: 'ghost', size: 'sm' }), className)}>
      {icon ?? <ArrowLeftIcon data-icon="inline-start" />}
      {children}
    </a>
  );
}

export const BackLink = createLink(BackAnchor);
