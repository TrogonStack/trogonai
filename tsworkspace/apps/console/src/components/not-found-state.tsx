import type { ReactNode } from 'react';

export function NotFoundState({ back, title, children }: { back: ReactNode; title: string; children: ReactNode }) {
  return (
    <div className="flex flex-col items-start gap-3">
      {back}
      <h1 className="text-xl font-semibold">{title}</h1>
      <p className="text-muted-foreground text-sm">{children}</p>
    </div>
  );
}
