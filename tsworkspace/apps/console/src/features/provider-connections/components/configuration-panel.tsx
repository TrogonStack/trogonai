import { OpenAiOutputLimitField } from '@trogonai/proto';
import { LockIcon } from 'lucide-react';
import type { ReactNode } from 'react';

import { CopyButton } from '@/components/copy-button';
import { Digest } from '@/components/digest';
import { Chip, ScopeChip } from '@/components/lifecycle-badge';
import { RegistrationMarks } from '@/components/spotlight-card';
import { Card, CardAction, CardContent, CardHeader, CardTitle } from '@/components/ui/card';

import { outputLimitLabels, protocolLabels } from '../domain/labels';
import { describeEndpoint, type ConnectionView } from '../domain/view';

function Row({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="border-border/70 grid grid-cols-1 gap-1 border-b py-2.5 last:border-b-0 sm:grid-cols-[10rem_minmax(0,1fr)] sm:gap-4">
      <dt className="micro text-muted-foreground pt-0.5 text-[10px]">{label}</dt>
      <dd className="min-w-0 text-[13px]">{children}</dd>
    </div>
  );
}

function Mono({ children, value, label }: { children: ReactNode; value?: string; label?: string }) {
  return (
    <span className="inline-flex max-w-full items-center gap-1 font-mono text-xs">
      <span className="break-all">{children}</span>
      {value && label ? <CopyButton value={value} label={label} /> : null}
    </span>
  );
}

export function ConfigurationPanel({ connection }: { connection: ConnectionView }) {
  return (
    <Card>
      <RegistrationMarks />
      <CardHeader>
        <CardTitle>Configuration</CardTitle>
        <CardAction>
          <span className="micro text-muted-foreground inline-flex items-center gap-1.5 text-[10px]">
            <LockIcon className="size-3" aria-hidden="true" />
            Immutable
          </span>
        </CardAction>
      </CardHeader>
      <CardContent className="flex flex-col gap-2">
        <p className="text-muted-foreground text-xs">
          This configuration is immutable. Changing any of it means registering a new connection.
        </p>
        <dl className="flex flex-col">
          <Row label="Connection id">
            <Mono value={connection.id} label="connection id">
              {connection.id}
            </Mono>
          </Row>
          <Row label="Parent node">
            <Mono>{connection.parentNodeId}</Mono>
          </Row>
          <Row label="Provider">
            <Mono>{connection.providerId}</Mono>
          </Row>
          <Row label="Endpoint">
            <Mono>{describeEndpoint(connection.endpoint)}</Mono>
          </Row>
          <Row label="Access scope">
            <ScopeChip scope={connection.accessScope} />
          </Row>
          <Row label="Protocols">
            <div className="flex flex-wrap gap-1">
              {connection.protocols.map((protocol) => (
                <Chip key={protocol}>{protocolLabels[protocol]}</Chip>
              ))}
            </div>
          </Row>
          {connection.openAiOutputLimit !== undefined ? (
            <Row label="OpenAI output limit">
              <Mono>{outputLimitLabels[connection.openAiOutputLimit as OpenAiOutputLimitField]}</Mono>
            </Row>
          ) : null}
          <Row label="Allowed models">
            <ul className="flex flex-col gap-2">
              {connection.models.map((model) => (
                <li
                  key={model.identifier}
                  className="border-border bg-surface-deep/50 flex flex-col gap-1 rounded-md border px-2.5 py-1.5"
                >
                  <span className="font-mono text-xs">
                    {model.identifier}
                    {model.providerRevision ? (
                      <span className="text-muted-foreground"> @ {model.providerRevision}</span>
                    ) : null}
                  </span>
                  {model.catalogPin ? (
                    <span className="text-muted-foreground flex flex-wrap items-center gap-x-2">
                      <span className="micro text-[9.5px]">Catalog pin</span>
                      <span className="font-mono text-xs">
                        {model.catalogPin.modelId}@{model.catalogPin.version}
                      </span>
                      <Digest hex={model.catalogPin.definitionDigestHex} label="catalog digest" />
                    </span>
                  ) : null}
                </li>
              ))}
            </ul>
          </Row>
          <Row label="Configuration digest">
            <span className="flex flex-wrap items-center gap-2">
              <Digest hex={connection.configurationDigestHex} label="configuration digest" />
              <span className="micro text-muted-foreground text-[10px]">
                {connection.configurationByteLength} bytes
              </span>
            </span>
          </Row>
        </dl>
      </CardContent>
    </Card>
  );
}
