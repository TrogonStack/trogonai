import { useNotify } from '@/components/ui/toaster';
import { cn } from '@/lib/utils';

import type { KeyView } from '../domain/view';
import { useSetDefaultKey } from '../queries';

export function DefaultSwitch({ view }: { view: KeyView }) {
  const setDefault = useSetDefaultKey();
  const notify = useNotify();
  const purpose = view.purposeLabel.toLowerCase();

  return (
    <div className="flex items-center gap-3">
      <button
        type="button"
        role="switch"
        aria-checked={view.isDefault}
        aria-label={`Default key for ${purpose}`}
        disabled={!view.makeDefault.available || setDefault.isPending}
        title={view.makeDefault.reason}
        onClick={() =>
          setDefault.mutate(view.keyRef, {
            onSuccess: () => notify.success('Default key changed', `${view.keyRef} now wraps new ${purpose}.`),
            onError: (error) => notify.failure('Could not change the default', error.message),
          })
        }
        className={cn(
          'focus-visible:ring-ring/70 relative inline-flex h-5 w-9 shrink-0 items-center rounded-full border transition-colors outline-none focus-visible:ring-2 disabled:cursor-not-allowed disabled:opacity-60',
          view.isDefault ? 'border-primary bg-primary' : 'border-border bg-muted',
        )}
      >
        <span
          aria-hidden="true"
          className={cn(
            'bg-background block size-3.5 rounded-full shadow transition-transform',
            view.isDefault ? 'translate-x-[18px]' : 'translate-x-0.5',
          )}
        />
      </button>
      <span className="flex flex-col">
        <span className="text-[13px] font-medium">Default for {purpose}</span>
        <span className="text-muted-foreground text-xs">
          {view.makeDefault.reason ??
            'Applies to new payloads only. Existing payloads keep their key until they are migrated.'}
        </span>
      </span>
    </div>
  );
}
