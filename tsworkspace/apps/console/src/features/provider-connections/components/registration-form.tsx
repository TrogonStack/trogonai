import { useForm } from '@tanstack/react-form';
import { Link } from '@tanstack/react-router';
import {
  OpenAiOutputLimitField,
  ProviderAccessScope,
  ProviderProtocol,
  ProviderRoutingEligibility,
} from '@trogonai/proto';
import { CircleAlertIcon, PlusIcon, SparklesIcon, Trash2Icon } from 'lucide-react';
import { useEffect, useState, type ReactNode } from 'react';

import { FaultAlert } from '@/components/fault-alert';
import { FieldError, FieldShell, SelectField, TextField, firstMessage, type SelectOption } from '@/components/field';
import { ProviderAvatar } from '@/components/provider-avatar';
import { SpotlightSurface } from '@/components/spotlight-card';
import { Button, buttonVariants } from '@/components/ui/button';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from '@/components/ui/dialog';
import { Segmented, type SegmentedOption } from '@/components/ui/segmented';
import { Textarea } from '@/components/ui/textarea';
import { cn } from '@/lib/utils';

import type { RegistrationDraft } from '../domain/draft';
import {
  accessScopeLabels,
  eligibilityLabels,
  outputLimitLabels,
  protocolLabels,
  selectableOutputLimits,
  selectableProtocols,
} from '../domain/labels';
import { applyPreset, presets, type Preset } from '../forms/presets';
import {
  emptyModelRow,
  emptyRegistrationValues,
  isOpenAiProtocolValue,
  registrationFormSchema,
  toRegistrationDraft,
  type RegistrationFormValues,
} from '../forms/registration';
import { BindingFields } from './binding-fields';

const accessScopeOptions: readonly SegmentedOption<string>[] = [
  ProviderAccessScope.MANAGED,
  ProviderAccessScope.GATEWAY_ONLY,
].map((scope) => ({ value: String(scope), label: accessScopeLabels[scope] }));

const endpointKindOptions: readonly SegmentedOption<RegistrationFormValues['endpointKind']>[] = [
  { value: 'network', label: 'Network URL' },
  { value: 'deployment-local', label: 'Deployment-local' },
];

const outputLimitOptions: readonly SelectOption<number>[] = [
  { value: OpenAiOutputLimitField.UNSPECIFIED, label: outputLimitLabels[OpenAiOutputLimitField.UNSPECIFIED] },
  ...selectableOutputLimits.map((limit) => ({ value: limit as number, label: outputLimitLabels[limit] })),
];

const eligibilityOptions: readonly SegmentedOption<string>[] = [
  ProviderRoutingEligibility.ENABLED,
  ProviderRoutingEligibility.DISABLED,
].map((value) => ({ value: String(value), label: eligibilityLabels[value] }));

const sections = [
  { id: 'preset', label: 'Preset' },
  { id: 'identity', label: 'Identity' },
  { id: 'endpoint', label: 'Endpoint' },
  { id: 'models', label: 'Models' },
  { id: 'protocols', label: 'Protocols' },
  { id: 'binding', label: 'Binding' },
  { id: 'profile', label: 'Profile' },
] as const;

interface RegistrationFormProps {
  onSubmit: (draft: RegistrationDraft) => Promise<unknown>;
  submitError?: unknown;
  initialValues?: RegistrationFormValues;
}

function Section({
  id,
  title,
  description,
  children,
}: {
  id: (typeof sections)[number]['id'];
  title: string;
  description?: string;
  children: ReactNode;
}) {
  return (
    <Card id={`section-${id}`} className="scroll-mt-20" aria-labelledby={`section-${id}-title`} role="group">
      <CardHeader className="flex-col items-start gap-1">
        <CardTitle id={`section-${id}-title`}>{title}</CardTitle>
        {description ? <CardDescription>{description}</CardDescription> : null}
      </CardHeader>
      <CardContent className="flex flex-col gap-4">{children}</CardContent>
    </Card>
  );
}

function SectionNav() {
  const [current, setCurrent] = useState<string>(sections[0].id);

  useEffect(() => {
    if (typeof IntersectionObserver === 'undefined') return;
    const visible = new Map<string, number>();
    const observer = new IntersectionObserver(
      (entries) => {
        for (const entry of entries) {
          const id = entry.target.id.replace('section-', '');
          if (entry.isIntersecting) visible.set(id, entry.boundingClientRect.top);
          else visible.delete(id);
        }
        const first = sections.find((section) => visible.has(section.id));
        if (first) setCurrent(first.id);
      },
      { rootMargin: '-80px 0px -55% 0px' },
    );
    for (const section of sections) {
      const element = document.getElementById(`section-${section.id}`);
      if (element) observer.observe(element);
    }
    return () => observer.disconnect();
  }, []);

  return (
    <nav aria-label="Form sections" className="sticky top-20 hidden self-start lg:block">
      <p className="micro text-muted-foreground mb-3 flex items-center gap-2 text-[10px]">
        <span aria-hidden="true" className="bg-muted-foreground/50 h-px w-2" />
        Sections
      </p>
      <ol className="border-border flex flex-col border-l">
        {sections.map((section, index) => {
          const active = current === section.id;
          return (
            <li key={section.id}>
              <a
                href={`#section-${section.id}`}
                aria-current={active ? 'location' : undefined}
                onClick={() => setCurrent(section.id)}
                className={cn(
                  'micro relative -ml-px flex h-8 items-center gap-2.5 border-l-2 px-3 text-[10.5px] transition-colors outline-none hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/70',
                  active ? 'border-primary text-foreground' : 'border-transparent text-muted-foreground',
                )}
              >
                <span className="text-muted-foreground/70 tabular-nums">{String(index + 1).padStart(2, '0')}</span>
                {section.label}
              </a>
            </li>
          );
        })}
      </ol>
    </nav>
  );
}

function presetHint(preset: Preset): string {
  const { baseUrl, endpointId } = preset.values;
  if (baseUrl) {
    try {
      return new URL(baseUrl).host;
    } catch {
      return baseUrl;
    }
  }
  return endpointId ? `local · ${endpointId}` : 'Blank form';
}

function PresetCard({
  providerId,
  label,
  hint,
  selected,
  onSelect,
}: {
  providerId: string;
  label: string;
  hint: string;
  selected: boolean;
  onSelect: () => void;
}) {
  return (
    <SpotlightSurface
      className={cn('transition-colors', selected ? 'border-primary bg-primary/5' : 'hover:border-foreground/25')}
    >
      <button
        type="button"
        aria-pressed={selected}
        onClick={onSelect}
        className="focus-visible:ring-ring/70 flex w-full items-center gap-3 p-3 text-left outline-none focus-visible:ring-2 focus-visible:ring-inset"
      >
        <ProviderAvatar providerId={providerId} name={label} />
        <span className="flex min-w-0 flex-col gap-0.5">
          <span className="truncate text-[13px] leading-4 font-medium">{label}</span>
          <span className="micro text-muted-foreground truncate text-[9.5px] tracking-wider normal-case">{hint}</span>
        </span>
        <span
          aria-hidden="true"
          className={cn(
            'led ml-auto size-1.5 shrink-0 rounded-full transition-opacity [--led:var(--primary)]',
            selected ? 'opacity-100' : 'opacity-0',
          )}
        />
      </button>
    </SpotlightSurface>
  );
}

const blankPreset = { providerId: 'blank', label: 'Start blank', hint: 'Empty form' } as const;

function PresetPicker({ presetKey, onChoose }: { presetKey: string; onChoose: (preset: Preset | undefined) => void }) {
  const [open, setOpen] = useState(false);
  const current = presets.find((preset) => preset.key === presetKey);
  const summary = current
    ? { providerId: current.values.providerId ?? current.key, label: current.label, hint: presetHint(current) }
    : blankPreset;

  const choose = (preset: Preset | undefined) => {
    onChoose(preset);
    setOpen(false);
  };

  return (
    <div className="flex items-center gap-3">
      <ProviderAvatar providerId={summary.providerId} name={summary.label} />
      <span className="flex min-w-0 flex-col gap-0.5">
        <span className="micro text-muted-foreground text-[9.5px]">Start from</span>
        <span className="truncate text-[13px] leading-4 font-medium">{summary.label}</span>
      </span>
      <Dialog open={open} onOpenChange={setOpen}>
        <DialogTrigger render={<Button type="button" variant="outline" size="sm" className="ml-auto" />}>
          <SparklesIcon data-icon="inline-start" />
          Browse presets
        </DialogTrigger>
        <DialogContent className="sm:max-w-3xl">
          <DialogHeader>
            <DialogTitle>Start from a preset</DialogTitle>
            <DialogDescription>A preset only prefills the form. Every value stays editable.</DialogDescription>
          </DialogHeader>
          <div role="group" aria-label="Start from" className="grid grid-cols-1 gap-2 sm:grid-cols-2 lg:grid-cols-3">
            <PresetCard
              providerId={blankPreset.providerId}
              label={blankPreset.label}
              hint={blankPreset.hint}
              selected={presetKey === ''}
              onSelect={() => choose(undefined)}
            />
            {presets.map((preset) => (
              <PresetCard
                key={preset.key}
                providerId={preset.values.providerId ?? preset.key}
                label={preset.label}
                hint={presetHint(preset)}
                selected={presetKey === preset.key}
                onSelect={() => choose(preset)}
              />
            ))}
          </div>
        </DialogContent>
      </Dialog>
    </div>
  );
}

function ProtocolChip({ label, checked, onToggle }: { label: string; checked: boolean; onToggle: () => void }) {
  return (
    <label className="group/chip border-input bg-card hover:bg-muted/60 has-checked:border-primary has-checked:bg-primary/8 has-focus-visible:ring-ring/70 flex h-9 cursor-pointer items-center gap-2.5 rounded-md border px-3 text-[13px] shadow-[inset_0_1px_0_0_var(--highlight)] transition-colors select-none has-focus-visible:ring-2">
      <input type="checkbox" className="peer sr-only" checked={checked} onChange={onToggle} />
      <span
        aria-hidden="true"
        className={cn(
          'size-1.5 shrink-0 rounded-full [--led:var(--primary)]',
          checked ? 'led' : 'bg-muted-foreground/30',
        )}
      />
      {label}
    </label>
  );
}

export function RegistrationForm({
  onSubmit,
  submitError,
  initialValues = emptyRegistrationValues,
}: RegistrationFormProps) {
  const [presetKey, setPresetKey] = useState('');
  const [defaultValues, setDefaultValues] = useState(initialValues);

  const form = useForm({
    defaultValues,
    validators: { onChange: registrationFormSchema },
    onSubmit: async ({ value }) => {
      const parsed = registrationFormSchema.safeParse(value);
      if (!parsed.success) return;
      await onSubmit(toRegistrationDraft(parsed.data));
    },
  });

  const choosePreset = (preset: Preset | undefined) => {
    setPresetKey(preset?.key ?? '');
    if (!preset) return;
    const next = applyPreset(form.state.values, preset);
    setDefaultValues(next);
    form.reset(next);
  };

  const fieldError = (field: { state: { meta: { isTouched: boolean; errors: readonly unknown[] } } }) =>
    field.state.meta.isTouched ? firstMessage(field.state.meta.errors) : undefined;

  return (
    <form
      aria-label="Register provider connection"
      noValidate
      onSubmit={(event) => {
        event.preventDefault();
        event.stopPropagation();
        void form.handleSubmit();
      }}
    >
      <div className="grid grid-cols-1 gap-8 lg:grid-cols-[10rem_minmax(0,1fr)]">
        <SectionNav />
        <div className="flex min-w-0 flex-col gap-5">
          <FaultAlert error={submitError} title="The registration was rejected" />

          <Section
            id="preset"
            title="Preset"
            description="A preset only prefills the form. Every value stays editable."
          >
            <PresetPicker presetKey={presetKey} onChoose={choosePreset} />
          </Section>

          <Section
            id="identity"
            title="Identity"
            description="Immutable once registered. Changing any of it means a new connection."
          >
            <div className="grid grid-cols-1 gap-3 md:grid-cols-3">
              <form.Field name="connectionId">
                {(field) => (
                  <TextField
                    id="connection-id"
                    label="Connection id"
                    value={field.state.value}
                    onChange={field.handleChange}
                    onBlur={field.handleBlur}
                    error={fieldError(field)}
                    mono
                    placeholder="openai-prod"
                  />
                )}
              </form.Field>
              <form.Field name="parentNodeId">
                {(field) => (
                  <TextField
                    id="parent-node-id"
                    label="Parent node id"
                    value={field.state.value}
                    onChange={field.handleChange}
                    onBlur={field.handleBlur}
                    error={fieldError(field)}
                    mono
                    placeholder="acme-platform"
                  />
                )}
              </form.Field>
              <form.Field name="providerId">
                {(field) => (
                  <TextField
                    id="provider-id"
                    label="Provider id"
                    value={field.state.value}
                    onChange={field.handleChange}
                    onBlur={field.handleBlur}
                    error={fieldError(field)}
                    mono
                    placeholder="openai"
                  />
                )}
              </form.Field>
            </div>
          </Section>

          <Section id="endpoint" title="Endpoint and access scope">
            <div className="grid grid-cols-1 gap-4 md:grid-cols-2">
              <form.Field name="endpointKind">
                {(field) => (
                  <div className="flex flex-col gap-1.5">
                    <span className="micro text-muted-foreground">Endpoint kind</span>
                    <Segmented
                      label="Endpoint kind"
                      value={field.state.value}
                      options={endpointKindOptions}
                      className="w-fit"
                      onChange={field.handleChange}
                    />
                  </div>
                )}
              </form.Field>
              <form.Field name="accessScope">
                {(field) => (
                  <div className="flex flex-col gap-1.5">
                    <span className="micro text-muted-foreground">Access scope</span>
                    <Segmented
                      label="Access scope"
                      value={String(field.state.value)}
                      options={accessScopeOptions}
                      className="w-fit"
                      onChange={(value) => field.handleChange(Number(value))}
                    />
                    {fieldError(field) ? <FieldError>{fieldError(field)}</FieldError> : null}
                  </div>
                )}
              </form.Field>
            </div>
            <form.Subscribe selector={(state) => state.values.endpointKind}>
              {(kind) =>
                kind === 'network' ? (
                  <form.Field name="baseUrl">
                    {(field) => (
                      <TextField
                        id="base-url"
                        label="Base URL"
                        value={field.state.value}
                        onChange={field.handleChange}
                        onBlur={field.handleBlur}
                        error={fieldError(field)}
                        mono
                        placeholder="https://api.openai.com/v1"
                        hint="https only, no credentials, query or fragment."
                      />
                    )}
                  </form.Field>
                ) : (
                  <form.Field name="endpointId">
                    {(field) => (
                      <TextField
                        id="endpoint-id"
                        label="Deployment endpoint id"
                        value={field.state.value}
                        onChange={field.handleChange}
                        onBlur={field.handleBlur}
                        error={fieldError(field)}
                        mono
                        placeholder="ollama-local"
                      />
                    )}
                  </form.Field>
                )
              }
            </form.Subscribe>
          </Section>

          <Section
            id="models"
            title="Allowed models"
            description="Provider model identifiers must be unique. Managed scope requires an exact catalog pin per model."
          >
            <form.Field name="models" mode="array">
              {(modelsField) => (
                <div className="flex flex-col gap-3">
                  {modelsField.state.value.map((_, index) => {
                    const ordinal = index + 1;
                    const modelLabel = (text: string) => (
                      <>
                        <span className="sr-only">Model {ordinal} </span>
                        {text}
                      </>
                    );
                    return (
                      <div
                        key={index}
                        role="group"
                        aria-label={`Model ${ordinal}`}
                        className="border-border bg-surface-deep/40 overflow-hidden rounded-md border"
                      >
                        <div className="border-border flex h-9 items-center justify-between border-b pr-1.5 pl-3">
                          <span className="micro text-muted-foreground text-[10px] tabular-nums">
                            Model {String(ordinal).padStart(2, '0')}
                          </span>
                          <Button
                            type="button"
                            variant="ghost"
                            size="icon-sm"
                            className="text-muted-foreground"
                            aria-label={`Remove model ${ordinal}`}
                            disabled={modelsField.state.value.length === 1}
                            onClick={() => void modelsField.removeValue(index)}
                          >
                            <Trash2Icon />
                          </Button>
                        </div>
                        <div className="grid grid-cols-1 gap-x-3 gap-y-4 p-3 md:grid-cols-2">
                          <form.Field name={`models[${index}].identifier`}>
                            {(field) => (
                              <TextField
                                id={`model-${index}-identifier`}
                                label={modelLabel('Identifier')}
                                value={field.state.value}
                                onChange={field.handleChange}
                                onBlur={field.handleBlur}
                                error={fieldError(field)}
                                mono
                                placeholder="gpt-4.1"
                              />
                            )}
                          </form.Field>
                          <form.Field name={`models[${index}].providerRevision`}>
                            {(field) => (
                              <TextField
                                id={`model-${index}-revision`}
                                label={modelLabel('Provider revision (optional)')}
                                value={field.state.value}
                                onChange={field.handleChange}
                                onBlur={field.handleBlur}
                                mono
                                placeholder="gpt-4.1-2025-04-14"
                              />
                            )}
                          </form.Field>
                          <form.Field name={`models[${index}].pinModelId`}>
                            {(field) => (
                              <TextField
                                id={`model-${index}-pin-model`}
                                label={modelLabel('Catalog model id')}
                                value={field.state.value}
                                onChange={field.handleChange}
                                onBlur={field.handleBlur}
                                error={fieldError(field)}
                                mono
                                placeholder="gpt-4.1"
                              />
                            )}
                          </form.Field>
                          <form.Field name={`models[${index}].pinVersion`}>
                            {(field) => (
                              <TextField
                                id={`model-${index}-pin-version`}
                                label={modelLabel('Catalog version')}
                                value={field.state.value}
                                onChange={field.handleChange}
                                onBlur={field.handleBlur}
                                error={fieldError(field)}
                                mono
                                placeholder="2025-04-14"
                              />
                            )}
                          </form.Field>
                          <form.Field name={`models[${index}].pinDigestHex`}>
                            {(field) => (
                              <TextField
                                id={`model-${index}-pin-digest`}
                                label={modelLabel('Definition digest')}
                                value={field.state.value}
                                onChange={field.handleChange}
                                onBlur={field.handleBlur}
                                error={fieldError(field)}
                                mono
                                placeholder="sha256 hex, 64 characters"
                                className="md:col-span-2"
                              />
                            )}
                          </form.Field>
                        </div>
                      </div>
                    );
                  })}
                  {firstMessage(modelsField.state.meta.errors) ? (
                    <FieldError>{firstMessage(modelsField.state.meta.errors)}</FieldError>
                  ) : null}
                  <div>
                    <Button
                      type="button"
                      variant="outline"
                      size="sm"
                      onClick={() => void modelsField.pushValue({ ...emptyModelRow })}
                    >
                      <PlusIcon data-icon="inline-start" />
                      Add model
                    </Button>
                  </div>
                </div>
              )}
            </form.Field>
          </Section>

          <Section id="protocols" title="Protocols" description="The wire protocols this connection can speak.">
            <form.Field name="protocols">
              {(field) => {
                const error = fieldError(field);
                return (
                  <div className="flex flex-col gap-2">
                    <div
                      role="group"
                      aria-label="Supported protocols"
                      className="grid grid-cols-1 gap-2 sm:grid-cols-2 xl:grid-cols-3"
                    >
                      {selectableProtocols.map((protocol: ProviderProtocol) => {
                        const checked = field.state.value.includes(protocol);
                        return (
                          <ProtocolChip
                            key={protocol}
                            label={protocolLabels[protocol]}
                            checked={checked}
                            onToggle={() => {
                              field.handleChange(
                                checked
                                  ? field.state.value.filter((value) => value !== protocol)
                                  : [...field.state.value, protocol],
                              );
                              field.handleBlur();
                            }}
                          />
                        );
                      })}
                    </div>
                    {error ? <FieldError>{error}</FieldError> : null}
                  </div>
                );
              }}
            </form.Field>
            <form.Subscribe selector={(state) => state.values.protocols.some(isOpenAiProtocolValue)}>
              {(hasOpenAi) =>
                hasOpenAi ? (
                  <form.Field name="outputLimit">
                    {(field) => (
                      <SelectField
                        id="output-limit"
                        label="OpenAI output limit field"
                        value={field.state.value}
                        options={outputLimitOptions}
                        onChange={field.handleChange}
                        className="max-w-xs"
                        hint="Which request field carries the output token limit."
                      />
                    )}
                  </form.Field>
                ) : null
              }
            </form.Subscribe>
          </Section>

          <Section
            id="binding"
            title="Initial binding"
            description="The first credential binding. It becomes active when the connection is activated."
          >
            <form.Subscribe
              selector={(state) => ({
                binding: state.values.binding,
                accessScope: state.values.accessScope,
                endpointKind: state.values.endpointKind,
                attempted: state.submissionAttempts > 0,
              })}
            >
              {({ binding, accessScope, endpointKind, attempted }) => (
                <BindingFields
                  idPrefix="initial"
                  value={binding}
                  context={{ accessScope, endpointKind }}
                  showErrors={attempted}
                  onChange={(next) => form.setFieldValue('binding', next)}
                />
              )}
            </form.Subscribe>
          </Section>

          <Section
            id="profile"
            title="Profile and routing policy"
            description="Editable later without creating a new connection."
          >
            <div className="grid grid-cols-1 gap-3 md:grid-cols-2">
              <form.Field name="displayName">
                {(field) => (
                  <TextField
                    id="display-name"
                    label="Display name"
                    value={field.state.value}
                    onChange={field.handleChange}
                    onBlur={field.handleBlur}
                    placeholder="OpenAI production"
                    className="md:col-span-2"
                  />
                )}
              </form.Field>
              <form.Field name="description">
                {(field) => (
                  <FieldShell id="description" label="Description" className="md:col-span-2">
                    <Textarea
                      id="description"
                      rows={3}
                      value={field.state.value}
                      onChange={(event) => field.handleChange(event.target.value)}
                      onBlur={field.handleBlur}
                    />
                  </FieldShell>
                )}
              </form.Field>
              <form.Field name="preference">
                {(field) => (
                  <TextField
                    id="preference"
                    label="Routing preference"
                    type="number"
                    value={field.state.value}
                    onChange={(value) => field.handleChange(value === '' ? Number.NaN : Number(value))}
                    onBlur={field.handleBlur}
                    error={fieldError(field)}
                    hint="Higher values are preferred when several connections can serve a request."
                  />
                )}
              </form.Field>
              <form.Field name="eligibility">
                {(field) => (
                  <div className="flex flex-col gap-1.5">
                    <span className="micro text-muted-foreground">Routing eligibility</span>
                    <Segmented
                      label="Routing eligibility"
                      value={String(field.state.value)}
                      options={eligibilityOptions}
                      className="w-fit"
                      onChange={(value) => field.handleChange(Number(value))}
                    />
                  </div>
                )}
              </form.Field>
            </div>
          </Section>
        </div>
      </div>

      <div className="border-border bg-background/85 sticky bottom-0 z-20 -mx-6 mt-6 border-t px-6 py-3 backdrop-blur-md">
        <div className="flex items-center justify-end gap-3">
          <form.Subscribe
            selector={(state) => ({
              attempted: state.submissionAttempts > 0,
              invalid: Object.values(state.fieldMeta).filter((meta) => (meta?.errors.length ?? 0) > 0).length,
            })}
          >
            {({ attempted, invalid }) =>
              attempted && invalid > 0 ? (
                <p role="status" className="text-danger-ink mr-auto flex items-center gap-2 text-xs">
                  <CircleAlertIcon className="size-3.5" aria-hidden="true" />
                  <span>
                    {invalid} {invalid === 1 ? 'field needs' : 'fields need'} attention
                  </span>
                </p>
              ) : null
            }
          </form.Subscribe>
          <Link to="/providers" className={buttonVariants({ variant: 'outline', size: 'lg' })}>
            Cancel
          </Link>
          <form.Subscribe selector={(state) => state.isSubmitting}>
            {(submitting) => (
              <Button type="submit" size="lg" disabled={submitting}>
                {submitting ? 'Registering...' : 'Register connection'}
              </Button>
            )}
          </form.Subscribe>
        </div>
      </div>
    </form>
  );
}
