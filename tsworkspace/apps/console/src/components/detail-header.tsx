import { Fragment, type ReactNode } from 'react';

import { CopyButton } from '@/components/copy-button';

interface DetailHeaderProps {
  leading?: ReactNode;
  title: string;
  status: ReactNode;
  titleAside?: ReactNode;
  identifier: ReactNode;
  copyValue: string;
  copyLabel: string;
  facts: readonly string[];
  chips?: ReactNode;
  chipsLabel?: string;
  actions: ReactNode;
}

export function DetailHeader({
  leading,
  title,
  status,
  titleAside,
  identifier,
  copyValue,
  copyLabel,
  facts,
  chips,
  chipsLabel,
  actions,
}: DetailHeaderProps) {
  return (
    <header className="flex flex-wrap items-start justify-between gap-4">
      <div className="flex min-w-0 items-center gap-4">
        {leading}
        <div className="flex min-w-0 flex-col gap-2">
          <div className="flex flex-wrap items-center gap-3">
            <h1 className="text-2xl leading-8 font-semibold">{title}</h1>
            {status}
            {titleAside}
          </div>
          <p className="text-muted-foreground flex min-w-0 flex-wrap items-center gap-2 text-xs">
            {identifier}
            <CopyButton value={copyValue} label={copyLabel} />
            {facts.map((fact) => (
              <Fragment key={fact}>
                <span aria-hidden="true">/</span>
                <span className="micro text-[10px]">{fact}</span>
              </Fragment>
            ))}
          </p>
          {chips ? (
            <div className="flex flex-wrap items-center gap-1.5" aria-label={chipsLabel}>
              {chips}
            </div>
          ) : null}
        </div>
      </div>
      {actions}
    </header>
  );
}
