import { PauseIcon, PlayIcon, RefreshCwIcon, WebhookIcon } from 'lucide-react';
import { useState } from 'react';

import { ConfirmDialog } from '@/components/confirm-dialog';
import { Button } from '@/components/ui/button';
import { useNotify } from '@/components/ui/toaster';

import type { SourceView } from '../domain/view';
import { usePauseSource, useReregisterWebhook, useResumeSource } from '../queries';
import { RotateSlotDialog } from './rotate-slot-dialog';

export function SourceActions({ source }: { source: SourceView }) {
  const pause = usePauseSource();
  const resume = useResumeSource();
  const reregister = useReregisterWebhook();
  const notify = useNotify();
  const [pausing, setPausing] = useState(false);
  const [rotating, setRotating] = useState(false);
  const [reregistering, setReregistering] = useState(false);

  const target = { integrationId: source.id, expectedRevision: source.revision };
  const { actions } = source;
  const hints = [actions.rotate.reason, actions.reregister.reason].filter((hint): hint is string => hint !== undefined);

  return (
    <div className="flex flex-col items-end gap-2">
      <div className="flex flex-wrap items-center gap-2">
        <Button
          type="button"
          variant="outline"
          size="lg"
          disabled={!actions.rotate.available}
          title={actions.rotate.reason}
          onClick={() => setRotating(true)}
        >
          <RefreshCwIcon data-icon="inline-start" />
          Rotate
        </Button>
        <Button
          type="button"
          variant="outline"
          size="lg"
          disabled={!actions.reregister.available}
          title={actions.reregister.reason}
          onClick={() => setReregistering(true)}
        >
          <WebhookIcon data-icon="inline-start" />
          Re-register webhook
        </Button>
        {actions.pause.available ? (
          <Button type="button" size="lg" variant="destructive" onClick={() => setPausing(true)}>
            <PauseIcon data-icon="inline-start" />
            Pause
          </Button>
        ) : null}
        {actions.resume.available ? (
          <Button
            type="button"
            size="lg"
            disabled={resume.isPending}
            onClick={() =>
              resume.mutate(target, {
                onSuccess: () => notify.success('Integration resumed', source.displayName),
                onError: (error) => notify.failure('Resume failed', error.message),
              })
            }
          >
            <PlayIcon data-icon="inline-start" />
            Resume
          </Button>
        ) : null}
      </div>
      {hints.length > 0 ? <p className="text-muted-foreground max-w-md text-right text-xs">{hints[0]}</p> : null}
      <ConfirmDialog
        open={pausing}
        onOpenChange={setPausing}
        icon={<PauseIcon />}
        title={`Pause ${source.displayName}?`}
        description="The gateway stops accepting deliveries from this integration until it is resumed. The provider may retry or drop what it cannot deliver."
        confirmLabel="Pause"
        onConfirm={() => {
          setPausing(false);
          pause.mutate(target, {
            onSuccess: () => notify.success('Integration paused', source.displayName),
            onError: (error) => notify.failure('Pause failed', error.message),
          });
        }}
      />
      <ConfirmDialog
        open={reregistering}
        onOpenChange={setReregistering}
        icon={<WebhookIcon />}
        title={`Re-register the webhook for ${source.displayName}?`}
        description="The provider is given the active secret version again, replacing whatever it holds now."
        confirmLabel="Re-register"
        onConfirm={() => {
          setReregistering(false);
          reregister.mutate(target, {
            onSuccess: () => notify.success('Re-registration started', source.displayName),
            onError: (error) => notify.failure('Re-registration failed', error.message),
          });
        }}
      />
      <RotateSlotDialog source={source} open={rotating} onOpenChange={setRotating} />
    </div>
  );
}
