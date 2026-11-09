import { Toast } from '@base-ui/react/toast';
import { CircleCheckIcon, TriangleAlertIcon, XIcon } from 'lucide-react';
import type { ReactNode } from 'react';

import { cn } from '@/lib/utils';

function ToastList() {
  const { toasts } = Toast.useToastManager();
  return toasts.map((toast) => {
    const failed = toast.type === 'error';
    return (
      <Toast.Root
        key={toast.id}
        toast={toast}
        className="animate-toast-in border-border bg-popover text-popover-foreground pointer-events-auto flex w-80 items-start gap-2.5 rounded-lg border p-3 shadow-lg transition-all data-ending-style:translate-y-2 data-ending-style:opacity-0"
      >
        <span
          className={cn(
            'mt-0.5 flex size-5 shrink-0 items-center justify-center rounded-full',
            failed ? 'bg-danger-soft text-danger-ink' : 'bg-success-soft text-success-ink',
          )}
        >
          {failed ? (
            <TriangleAlertIcon className="size-3" aria-hidden="true" />
          ) : (
            <CircleCheckIcon className="size-3" aria-hidden="true" />
          )}
        </span>
        <div className="flex min-w-0 flex-1 flex-col gap-0.5">
          <Toast.Title className="text-[13px] font-medium" />
          <Toast.Description className="text-muted-foreground text-xs" />
        </div>
        <Toast.Close
          aria-label="Dismiss notification"
          className="text-muted-foreground hover:bg-muted hover:text-foreground focus-visible:ring-ring/60 -mt-0.5 -mr-1 flex size-6 shrink-0 items-center justify-center rounded-md outline-none focus-visible:ring-2"
        >
          <XIcon className="size-3.5" aria-hidden="true" />
        </Toast.Close>
      </Toast.Root>
    );
  });
}

export function ToastProvider({ children }: { children: ReactNode }) {
  return (
    <Toast.Provider timeout={4000} limit={3}>
      {children}
      <Toast.Portal>
        <Toast.Viewport className="pointer-events-none fixed right-4 bottom-4 z-[60] flex flex-col items-end gap-2">
          <ToastList />
        </Toast.Viewport>
      </Toast.Portal>
    </Toast.Provider>
  );
}

export function useNotify() {
  const manager = Toast.useToastManager();
  return {
    success: (title: string, description?: string) => manager.add({ title, description, type: 'success' }),
    failure: (title: string, description?: string) => manager.add({ title, description, type: 'error' }),
  };
}
