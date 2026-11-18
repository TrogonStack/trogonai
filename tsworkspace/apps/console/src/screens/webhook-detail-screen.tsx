import { useNavigate } from '@tanstack/react-router';
import { Settings2Icon } from 'lucide-react';
import { useState } from 'react';

import { BackLink } from '@/components/back-link';
import { DetailHeader } from '@/components/detail-header';
import { FaultAlert } from '@/components/fault-alert';
import { NotFoundState } from '@/components/not-found-state';
import { Button } from '@/components/ui/button';
import { Card, CardContent } from '@/components/ui/card';
import { CustodyTierBadge } from '@/features/custody/components/custody-tier-badge';
import { WebhookEndpointControls } from '@/features/gateway-sources/components/generic-webhooks';
import { SourceKindIcon } from '@/features/gateway-sources/components/source-kind-icon';
import { PreviewBadge, SourceStatusBadge } from '@/features/gateway-sources/components/status-badges';
import { WebhookFormDialog } from '@/features/gateway-sources/components/webhook-form';
import { webhookSourceRow } from '@/features/gateway-sources/domain/source-list';
import { webhookSubject } from '@/features/gateway-sources/domain/webhook-endpoint';
import {
  useWebhookEndpointsQuery,
  useWebhookMutation,
  type WebhookMutation,
} from '@/features/gateway-sources/webhook-queries';
import { MetaChip } from '@/features/secrets/components/badges';

export function WebhookDetailScreen({ endpointId }: { endpointId: string }) {
  const query = useWebhookEndpointsQuery();
  const mutation = useWebhookMutation();
  const navigate = useNavigate();
  const [editing, setEditing] = useState(false);
  const back = <BackLink to="/sources">All sources</BackLink>;
  if (query.isPending)
    return (
      <p role="status" className="text-muted-foreground text-sm">
        Loading source...
      </p>
    );
  if (query.isError) return <FaultAlert error={query.error} title="Could not load the source" />;
  const endpoint = query.data.find((entry) => entry.id === endpointId);
  if (!endpoint)
    return (
      <NotFoundState back={back} title="Source not found">
        No generic webhook endpoint has this ID in the selected project.
      </NotFoundState>
    );
  const source = webhookSourceRow(endpoint, new Date());
  const onCommand = (command: WebhookMutation) =>
    mutation.mutate(command, {
      onSuccess: () => {
        if (command.case === 'delete') void navigate({ to: '/sources' });
      },
    });
  return (
    <div className="flex flex-col gap-6">
      <div>{back}</div>
      <DetailHeader
        leading={<SourceKindIcon kind="webhook" className="size-12" />}
        title={endpoint.displayName}
        status={<SourceStatusBadge status={source.status} />}
        titleAside={
          <>
            <CustodyTierBadge tier="managed" />
            <PreviewBadge />
          </>
        }
        identifier={<span className="font-mono">{endpoint.id}</span>}
        copyValue={endpoint.id}
        copyLabel="endpoint id"
        facts={[
          'Generic webhook',
          endpoint.authentication.mode === 'hmac' ? 'HMAC-SHA256' : 'Bearer token',
          `Revision ${endpoint.configurationRevision}`,
        ]}
        chipsLabel="Routing"
        chips={
          <MetaChip title="Consumer subject" mono>
            {webhookSubject(endpoint)}
          </MetaChip>
        }
        actions={
          <Button
            type="button"
            variant="outline"
            disabled={mutation.isPending}
            onClick={() => {
              mutation.reset();
              setEditing(true);
            }}
          >
            <Settings2Icon />
            Settings
          </Button>
        }
      />
      <p className="text-muted-foreground text-xs">
        Interactive preview. Endpoint settings, credentials and simulated deliveries reset on reload.
      </p>
      {!editing ? <FaultAlert error={mutation.error} title="Endpoint action failed" /> : null}
      <Card>
        <CardContent className="pt-5">
          <WebhookEndpointControls
            key={endpoint.id}
            endpoint={endpoint}
            pending={mutation.isPending}
            onCommand={onCommand}
          />
        </CardContent>
      </Card>
      <WebhookFormDialog
        open={editing}
        endpoint={endpoint}
        pending={mutation.isPending}
        error={mutation.error}
        onOpenChange={(open) => {
          setEditing(open);
          if (!open) mutation.reset();
        }}
        onSubmit={async (draft) => {
          try {
            await mutation.mutateAsync({
              case: 'update',
              target: { id: endpoint.id, expectedRevision: endpoint.revision },
              draft,
            });
            setEditing(false);
          } catch {
            return;
          }
        }}
      />
    </div>
  );
}
