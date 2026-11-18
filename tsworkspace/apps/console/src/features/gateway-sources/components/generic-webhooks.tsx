import { useNavigate } from '@tanstack/react-router';
import { CopyIcon, PauseIcon, PlayIcon, PlusIcon, RefreshCwIcon, ShieldOffIcon, Trash2Icon } from 'lucide-react';
import { useState } from 'react';

import { ConfirmDialog } from '@/components/confirm-dialog';
import { FaultAlert } from '@/components/fault-alert';
import { Button } from '@/components/ui/button';
import { useCurrentProjectId } from '@/features/projects/current-project';

import { WEBHOOK_EXAMPLE_BODY, webhookSubject, webhookUrl, type WebhookEndpoint } from '../domain/webhook-endpoint';
import { useWebhookMutation, type WebhookMutation } from '../webhook-queries';
import { WebhookDeliveries } from './webhook-deliveries';
import { WebhookFormDialog } from './webhook-form';

function RequestExample({ endpoint }: { endpoint: WebhookEndpoint }) {
  const authentication =
    endpoint.authentication.mode === 'hmac'
      ? 'webhook-timestamp: <current Unix seconds>\nwebhook-signature: v1,<computed HMAC-SHA256 in base64>'
      : 'Authorization: Bearer <WEBHOOK_SECRET>';
  return (
    <details>
      <summary className="cursor-pointer text-xs font-medium">How to send a request</summary>
      <div className="flex flex-col gap-3 pt-3">
        <pre className="bg-muted/50 overflow-x-auto rounded-md p-3 font-mono text-xs leading-5 break-all whitespace-pre-wrap">{`POST /webhooks/${endpoint.id}\nContent-Type: application/json\nwebhook-id: order-1042-created\n${authentication}\n\n${WEBHOOK_EXAMPLE_BODY}`}</pre>
        {endpoint.authentication.mode === 'hmac' ? (
          <p className="text-muted-foreground text-xs">
            The credential is whsec_ followed by a base64-encoded 32-byte key. Decode that key and use its bytes to
            HMAC-SHA256 delivery ID + "." + timestamp + "." + the exact original body bytes. Send v1, followed by the
            base64-encoded digest. This example contains placeholders only.
          </p>
        ) : null}
        <p className="text-muted-foreground text-xs">
          A 202 response acknowledges durable receipt, including a recognized retry. Your consumer owns processing and
          idempotent business effects.
        </p>
      </div>
    </details>
  );
}

export function WebhookEndpointControls({
  endpoint,
  pending,
  onCommand,
}: {
  endpoint: WebhookEndpoint;
  pending: boolean;
  onCommand: (command: WebhookMutation) => void;
}) {
  const [confirmation, setConfirmation] = useState<'pause' | 'rotate' | 'revoke' | 'delete'>();
  const [copied, setCopied] = useState(false);
  const [copyError, setCopyError] = useState<Error>();
  const target = { id: endpoint.id, expectedRevision: endpoint.revision };
  const confirmationCopy = {
    pause: {
      title: `Pause ${endpoint.displayName}?`,
      text: 'Subsequent simulated requests receive 404. Events already received remain retained.',
      label: 'Pause endpoint',
      icon: <PauseIcon />,
    },
    rotate: {
      title: 'Rotate webhook credential?',
      text: 'Advance the preview credential version. In a live integration, the sender must receive the new credential through the credential management flow.',
      label: 'Rotate credential',
      icon: <RefreshCwIcon />,
    },
    revoke: {
      title: 'Revoke webhook credential?',
      text: 'Subsequent simulated requests receive 401. Revocation does not delete previously stored events. This preview does not change a real credential.',
      label: 'Revoke credential',
      icon: <ShieldOffIcon />,
    },
    delete: {
      title: `Delete ${endpoint.displayName}?`,
      text: 'Remove this endpoint and its simulated history from this preview. In a live gateway, removing the registry entry does not erase stored deliveries or revoke its separate credential.',
      label: 'Delete endpoint',
      icon: <Trash2Icon />,
    },
  };
  const confirm = confirmation ? confirmationCopy[confirmation] : undefined;
  return (
    <div className="flex min-w-0 flex-col gap-5">
      <div className="flex min-w-0 flex-col gap-2">
        <div className="flex flex-wrap items-center justify-between gap-2">
          <span className="micro text-muted-foreground text-[10px]">Example endpoint URL</span>
          <Button
            type="button"
            variant="ghost"
            size="sm"
            onClick={() => {
              if (!navigator.clipboard) {
                setCopyError(
                  new Error('Clipboard access is unavailable. Select the example URL below to copy it manually.'),
                );
                return;
              }
              void navigator.clipboard
                .writeText(webhookUrl(endpoint))
                .then(() => {
                  setCopied(true);
                  setCopyError(undefined);
                })
                .catch(() =>
                  setCopyError(new Error('Could not copy. Select the example URL below to copy it manually.')),
                );
            }}
          >
            <CopyIcon />
            {copied ? 'Copied example URL' : 'Copy example URL'}
          </Button>
        </div>
        <code className="border-border bg-muted/35 rounded-md border p-3 text-xs break-all">
          {webhookUrl(endpoint)}
        </code>
        <p className="text-muted-foreground text-xs">
          Example address only. This preview is not connected to a running gateway.
        </p>
        <FaultAlert error={copyError} title="Copy failed" />
      </div>
      <dl className="grid grid-cols-2 gap-x-5 gap-y-3 text-xs xl:grid-cols-3">
        <div className="flex flex-col gap-1">
          <dt className="text-muted-foreground">Body limit</dt>
          <dd>{endpoint.maxBodyBytes.toLocaleString()} bytes</dd>
        </div>
        {endpoint.authentication.mode === 'hmac' ? (
          <div className="flex flex-col gap-1">
            <dt className="text-muted-foreground">Timestamp tolerance</dt>
            <dd>{endpoint.authentication.timestampToleranceSeconds} seconds</dd>
          </div>
        ) : null}
        <div className="flex flex-col gap-1">
          <dt className="text-muted-foreground">Configuration</dt>
          <dd>Revision {endpoint.configurationRevision}</dd>
        </div>
      </dl>
      <div className="border-border flex flex-wrap items-center justify-between gap-3 border-y py-3">
        <div className="flex flex-col gap-1">
          <span className="text-xs font-medium">Webhook credential</span>
          <span className="text-muted-foreground text-xs">
            {endpoint.credential.status === 'active'
              ? `Active preview version ${endpoint.credential.version}`
              : 'Revoked'}{' '}
            · Managed key
          </span>
          <span className="text-muted-foreground text-xs">
            Credential state only. No secret material is created or displayed.
          </span>
        </div>
        <div className="flex flex-wrap gap-2">
          <Button
            type="button"
            size="sm"
            variant="outline"
            disabled={pending || endpoint.credential.status === 'revoked'}
            onClick={() => setConfirmation('rotate')}
          >
            <RefreshCwIcon />
            Rotate
          </Button>
          <Button
            type="button"
            size="sm"
            variant="outline"
            disabled={pending || endpoint.credential.status === 'revoked'}
            onClick={() => setConfirmation('revoke')}
          >
            <ShieldOffIcon />
            Revoke
          </Button>
        </div>
      </div>
      <RequestExample endpoint={endpoint} />
      <div className="flex flex-col gap-1.5">
        <span className="micro text-muted-foreground text-[10px]">Consumer subject</span>
        <code className="text-xs break-all">{webhookSubject(endpoint)}</code>
        <p className="text-muted-foreground text-xs">
          The project and endpoint determine the subject. Senders cannot choose a destination or grant execution
          permissions.
        </p>
      </div>
      <div className="border-border border-t pt-5">
        <WebhookDeliveries
          key={endpoint.id}
          endpoint={endpoint}
          pending={pending}
          onTest={(request) => onCommand({ case: 'test', id: endpoint.id, request })}
        />
      </div>
      <div className="border-border flex flex-wrap items-center justify-between gap-3 border-t pt-4">
        <Button
          type="button"
          variant="outline"
          size="sm"
          disabled={pending}
          onClick={() =>
            endpoint.enabled ? setConfirmation('pause') : onCommand({ case: 'enable', target, enabled: true })
          }
        >
          {endpoint.enabled ? <PauseIcon /> : <PlayIcon />}
          {endpoint.enabled ? 'Pause endpoint' : 'Enable endpoint'}
        </Button>
        <Button
          type="button"
          variant="ghost"
          size="sm"
          className="text-danger-ink"
          disabled={pending}
          onClick={() => setConfirmation('delete')}
        >
          <Trash2Icon />
          Delete endpoint
        </Button>
      </div>
      {confirm ? (
        <ConfirmDialog
          open
          onOpenChange={(open) => {
            if (!open) setConfirmation(undefined);
          }}
          icon={confirm.icon}
          title={confirm.title}
          description={confirm.text}
          confirmLabel={confirm.label}
          onConfirm={() => {
            if (confirmation === 'pause') onCommand({ case: 'enable', target, enabled: false });
            else if (confirmation) onCommand({ case: confirmation, target });
            setConfirmation(undefined);
          }}
        />
      ) : null}
    </div>
  );
}

function CreateWebhookButtonInner() {
  const navigate = useNavigate();
  const mutation = useWebhookMutation();
  const [open, setOpen] = useState(false);
  const project = useCurrentProjectId();
  return (
    <>
      <Button
        type="button"
        disabled={!project}
        onClick={() => {
          mutation.reset();
          setOpen(true);
        }}
      >
        <PlusIcon />
        Create webhook
      </Button>
      <WebhookFormDialog
        open={open}
        pending={mutation.isPending}
        error={mutation.error}
        onOpenChange={(next) => {
          setOpen(next);
          if (!next) mutation.reset();
        }}
        onSubmit={async (draft) => {
          try {
            const result = await mutation.mutateAsync({ case: 'create', draft });
            if (result && 'id' in result) {
              setOpen(false);
              await navigate({ to: '/sources/webhooks/$endpointId', params: { endpointId: result.id } });
            }
          } catch {
            return;
          }
        }}
      />
    </>
  );
}

export function CreateWebhookButton() {
  const project = useCurrentProjectId();
  return <CreateWebhookButtonInner key={project} />;
}
