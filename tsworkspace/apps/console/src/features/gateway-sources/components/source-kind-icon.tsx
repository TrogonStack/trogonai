import {
  ActivityIcon,
  AtSignIcon,
  BugIcon,
  GitBranchIcon,
  GitMergeIcon,
  HashIcon,
  MailIcon,
  MessagesSquareIcon,
  NotebookTextIcon,
  RadioTowerIcon,
  SendIcon,
  SirenIcon,
  SquareKanbanIcon,
  VideoIcon,
  type LucideIcon,
} from 'lucide-react';

import { cn } from '@/lib/utils';

import type { SourceKind } from '../domain/source-kind';

const icons: Record<SourceKind, LucideIcon> = {
  github: GitBranchIcon,
  gitlab: GitMergeIcon,
  slack: HashIcon,
  discord: MessagesSquareIcon,
  telegram: SendIcon,
  linear: SquareKanbanIcon,
  notion: NotebookTextIcon,
  sentry: BugIcon,
  datadog: ActivityIcon,
  incidentio: SirenIcon,
  microsoft_graph: MailIcon,
  google_pubsub: RadioTowerIcon,
  twitter: AtSignIcon,
  zoom: VideoIcon,
};

export function SourceKindIcon({ kind, className }: { kind: SourceKind; className?: string }) {
  const Icon = icons[kind];
  return (
    <span className={cn('keycap flex size-8 shrink-0 items-center justify-center rounded-md', className)}>
      <Icon className="size-4" aria-hidden="true" />
    </span>
  );
}
