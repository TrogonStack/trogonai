import { MoreHorizontalIcon, ShieldAlertIcon, Trash2Icon } from 'lucide-react';
import { useState } from 'react';

import { BackLink } from '@/components/back-link';
import { ConfirmDialog } from '@/components/confirm-dialog';
import { CopyButton } from '@/components/copy-button';
import { CountTag } from '@/components/count-tag';
import { DetailHeader } from '@/components/detail-header';
import { Digest } from '@/components/digest';
import { FaultAlert } from '@/components/fault-alert';
import { BindingLifecycleBadge, ConnectionLifecycleBadge } from '@/components/lifecycle-badge';
import { NotFoundState } from '@/components/not-found-state';
import { ProviderAvatar } from '@/components/provider-avatar';
import { ScrambleText } from '@/components/scramble-text';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu';
import { Skeleton } from '@/components/ui/skeleton';
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs';
import { useNotify } from '@/components/ui/toaster';
import { BindingsPanel } from '@/features/provider-connections/components/bindings-panel';
import { ConfigurationPanel } from '@/features/provider-connections/components/configuration-panel';
import { HistoryTimeline } from '@/features/provider-connections/components/history-timeline';
import { ProfileForm, RoutingPolicyForm } from '@/features/provider-connections/components/profile-policy-form';
import { ReplaceBindingDialog } from '@/features/provider-connections/components/replace-binding-dialog';
import {
  canActivate,
  describeAuthentication,
  isActiveConnection,
  isLiveConnection,
  type ConnectionView,
} from '@/features/provider-connections/domain/view';
import {
  useActivateConnection,
  useConnectionHistoryQuery,
  useConnectionQuery,
  useRevokeConnection,
} from '@/features/provider-connections/queries';

function ConnectionActions({ connection }: { connection: ConnectionView }) {
  const activate = useActivateConnection();
  const revoke = useRevokeConnection();
  const notify = useNotify();
  const [confirming, setConfirming] = useState(false);
  const [replacing, setReplacing] = useState(false);

  const target = { connectionId: connection.id, expectedRevision: connection.revision };
  const name = connection.displayName || connection.id;
  const replaceable = isActiveConnection(connection) ? connection.activeBinding : undefined;

  return (
    <div className="flex flex-col items-end gap-2">
      <div className="flex flex-wrap items-center gap-2">
        {canActivate(connection) ? (
          <Button
            type="button"
            size="lg"
            disabled={activate.isPending}
            onClick={() =>
              activate.mutate(target, {
                onSuccess: () => notify.success('Connection activated', name),
                onError: (error) => notify.failure('Activation failed', error.message),
              })
            }
          >
            {activate.isPending ? 'Activating...' : 'Activate'}
          </Button>
        ) : null}
        {replaceable ? (
          <Button type="button" size="lg" onClick={() => setReplacing(true)}>
            Replace binding
          </Button>
        ) : null}
        {isLiveConnection(connection) ? (
          <DropdownMenu>
            <DropdownMenuTrigger
              render={<Button type="button" variant="outline" size="icon-lg" aria-label="Connection actions" />}
            >
              <MoreHorizontalIcon />
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end" className="w-52">
              <DropdownMenuItem variant="destructive" onClick={() => setConfirming(true)}>
                <Trash2Icon />
                Revoke connection
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        ) : null}
      </div>
      <FaultAlert error={activate.error ?? revoke.error} />
      {replaceable ? (
        <ReplaceBindingDialog
          connection={connection}
          previous={replaceable}
          open={replacing}
          onOpenChange={setReplacing}
        />
      ) : null}
      <ConfirmDialog
        open={confirming}
        onOpenChange={setConfirming}
        icon={<ShieldAlertIcon />}
        title={`Revoke ${name}?`}
        description="Revoking is permanent. The connection and all of its bindings stop serving requests and cannot be reinstated."
        confirmLabel="Revoke connection"
        onConfirm={() => {
          revoke.mutate(target, {
            onSuccess: () => notify.success('Connection revoked', name),
            onError: (error) => notify.failure('Revoke failed', error.message),
          });
          setConfirming(false);
        }}
      />
    </div>
  );
}

function ActiveBindingCard({ connection }: { connection: ConnectionView }) {
  const binding = connection.activeBinding;
  const secretRef = binding && 'secretRef' in binding.authentication ? binding.authentication.secretRef : undefined;
  return (
    <Card>
      <CardHeader>
        <CardTitle>Active binding</CardTitle>
      </CardHeader>
      <CardContent className="flex flex-col gap-3">
        {binding ? (
          <>
            <div className="flex flex-wrap items-center justify-between gap-2">
              <ScrambleText text={binding.id} className="text-sm font-medium" />
              <BindingLifecycleBadge lifecycle={binding.lifecycle} />
            </div>
            <dl className="flex flex-col gap-2.5">
              <div className="flex flex-col gap-1">
                <dt className="micro text-muted-foreground text-[10px]">Authentication</dt>
                <dd className="text-[13px]">{describeAuthentication(binding.authentication)}</dd>
              </div>
              {secretRef ? (
                <div className="flex flex-col gap-1">
                  <dt className="micro text-muted-foreground text-[10px]">Secret reference</dt>
                  <dd className="inline-flex min-w-0 items-center gap-1 font-mono text-xs">
                    <span className="truncate" title={secretRef}>
                      {secretRef}
                    </span>
                    <CopyButton value={secretRef} label="secret reference" />
                  </dd>
                </div>
              ) : null}
              <div className="flex flex-col gap-1">
                <dt className="micro text-muted-foreground text-[10px]">Digest</dt>
                <dd>
                  <Digest hex={binding.digestHex} label="active binding digest" />
                </dd>
              </div>
            </dl>
          </>
        ) : (
          <p className="text-muted-foreground text-[13px]">
            None. This connection has no active binding. Activate it once the pending binding validates.
          </p>
        )}
      </CardContent>
    </Card>
  );
}

function DetailSkeleton() {
  return (
    <div role="status" aria-busy="true" className="flex flex-col gap-6">
      <span className="sr-only">Loading connection</span>
      <div className="flex items-center gap-4">
        <Skeleton className="size-12" />
        <div className="flex flex-col gap-2">
          <Skeleton className="h-6 w-64" />
          <Skeleton className="h-3 w-40" />
        </div>
      </div>
      <Skeleton className="h-10 w-full" />
      <div className="grid gap-4 xl:grid-cols-[minmax(0,1fr)_22rem]">
        <Skeleton className="h-96" />
        <div className="flex flex-col gap-4">
          <Skeleton className="h-52" />
          <Skeleton className="h-44" />
        </div>
      </div>
    </div>
  );
}

export function ProviderDetailScreen({ connectionId }: { connectionId: string }) {
  const query = useConnectionQuery(connectionId);
  const history = useConnectionHistoryQuery(connectionId);

  const back = <BackLink to="/providers">All connections</BackLink>;

  if (query.isPending) return <DetailSkeleton />;

  if (query.isError) return <FaultAlert error={query.error} title="Could not load the connection" />;

  const connection = query.data;
  if (!connection) {
    return (
      <NotFoundState back={back} title="Connection not found">
        No provider connection has the id <span className="font-mono">{connectionId}</span>.
      </NotFoundState>
    );
  }

  const editable = isLiveConnection(connection);

  return (
    <div className="flex flex-col gap-6">
      <div>{back}</div>
      <DetailHeader
        leading={
          <ProviderAvatar
            providerId={connection.providerId}
            name={connection.displayName}
            size="lg"
            muted={!editable}
          />
        }
        title={connection.displayName || connection.id}
        status={<ConnectionLifecycleBadge lifecycle={connection.lifecycle} />}
        identifier={<ScrambleText text={connection.id} />}
        copyValue={connection.id}
        copyLabel="connection id"
        facts={[`Revision ${connection.revision}`]}
        actions={<ConnectionActions connection={connection} />}
      />

      <Tabs defaultValue="overview">
        <TabsList>
          <TabsTrigger value="overview">Overview</TabsTrigger>
          <TabsTrigger value="bindings">
            Bindings
            <CountTag count={connection.bindings.length} />
          </TabsTrigger>
          <TabsTrigger value="history">
            History
            <CountTag count={history.data?.length} />
          </TabsTrigger>
        </TabsList>
        <TabsContent value="overview">
          <div className="grid items-start gap-4 xl:grid-cols-[minmax(0,1fr)_22rem]">
            <ConfigurationPanel connection={connection} />
            <div className="flex flex-col gap-4">
              <Card>
                <CardHeader>
                  <CardTitle>Profile</CardTitle>
                </CardHeader>
                <CardContent>
                  <ProfileForm connection={connection} editable={editable} />
                </CardContent>
              </Card>
              <Card>
                <CardHeader>
                  <CardTitle>Routing policy</CardTitle>
                </CardHeader>
                <CardContent>
                  <RoutingPolicyForm connection={connection} editable={editable} />
                </CardContent>
              </Card>
              <ActiveBindingCard connection={connection} />
            </div>
          </div>
        </TabsContent>
        <TabsContent value="bindings">
          <BindingsPanel connection={connection} />
        </TabsContent>
        <TabsContent value="history">
          {history.data ? (
            <HistoryTimeline events={history.data} />
          ) : (
            <Skeleton role="status" aria-label="Loading history" className="h-48 w-full" />
          )}
        </TabsContent>
      </Tabs>
    </div>
  );
}
