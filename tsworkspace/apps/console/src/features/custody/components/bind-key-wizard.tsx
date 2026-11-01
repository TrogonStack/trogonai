import { Link } from '@tanstack/react-router';
import { KeyBackendAdmission, KeyBackendLifecycle, KeyCapability } from '@trogonai/proto';
import { ArrowLeftIcon, ArrowRightIcon, CheckCircle2Icon } from 'lucide-react';
import { useState } from 'react';

import { CopyButton } from '@/components/copy-button';
import { FaultAlert } from '@/components/fault-alert';
import { FieldError, RadioCards, TextField, type RadioCardOption } from '@/components/field';
import { Button, buttonVariants } from '@/components/ui/button';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import { Label } from '@/components/ui/label';
import { useNotify } from '@/components/ui/toaster';

import { keyCapabilityDescriptions, keyCapabilityLabels } from '../domain/capability';
import type { KeyRef, PurposeId } from '../domain/identifiers';
import { locatorHints, locatorPlaceholders, locatorProblem } from '../domain/locator';
import { keyProviderLabels } from '../domain/provider';
import type { BindingPlan } from '../domain/validation';
import type { BackendView } from '../domain/view';
import { useCompleteBinding, useCustodyQuery, usePrepareBinding } from '../queries';
import { StepIndicator } from './step-indicator';
import { useStagedChecks, ValidationChecklist } from './validation-checklist';

const steps = ['Backend', 'Key', 'Validate', 'Done'] as const;

const requestableCapabilities = [KeyCapability.ENVELOPE_WRAPPING] as const;

function ValidateStep({
  plan,
  onBack,
  onDone,
}: {
  plan: BindingPlan;
  onBack: () => void;
  onDone: (keyRef: KeyRef) => void;
}) {
  const staged = useStagedChecks(plan.attemptId, plan.checks);
  const complete = useCompleteBinding();
  const notify = useNotify();

  const finish = async () => {
    try {
      const key = await complete.mutateAsync(plan.attemptId);
      notify.success('Key bound', key.keyRef);
      onDone(key.keyRef);
    } catch {
      return;
    }
  };

  return (
    <div className="flex flex-col gap-4">
      <ValidationChecklist
        title="Validation"
        description="The platform proves it can use exactly this key before it issues a reference."
        checks={plan.checks}
        staged={staged}
      />
      {staged.failed ? (
        <p
          role="status"
          className="border-warning/35 bg-warning-soft text-warning-ink rounded-lg border px-4 py-3 text-[13px]"
        >
          Fix the failing item at your provider, then run the checks again.
        </p>
      ) : null}
      <FaultAlert error={complete.error} title="The binding was rejected" />
      <div className="flex flex-wrap items-center justify-between gap-2">
        <Button type="button" variant="outline" disabled={staged.running} onClick={onBack}>
          <ArrowLeftIcon data-icon="inline-start" />
          Back
        </Button>
        <Button type="button" disabled={!staged.allPassed || complete.isPending} onClick={() => void finish()}>
          {complete.isPending ? 'Binding...' : 'Bind key'}
        </Button>
      </div>
    </div>
  );
}

function describeBackend(backend: BackendView): string {
  const base = `${keyProviderLabels[backend.provider]}. ${backend.profileSummary}.`;
  return backend.admission === KeyBackendAdmission.PAUSED ? `${base} Paused.` : base;
}

export function BindKeyWizard({ initialBackendId }: { initialBackendId?: string }) {
  const query = useCustodyQuery();
  const prepare = usePrepareBinding();
  const [step, setStep] = useState(0);
  const [chosenBackend, setChosenBackend] = useState<string | undefined>(initialBackendId);
  const [locator, setLocator] = useState('');
  const [chosenPurpose, setChosenPurpose] = useState<PurposeId>();
  const [capabilities, setCapabilities] = useState<readonly KeyCapability[]>([KeyCapability.ENVELOPE_WRAPPING]);
  const [showProblems, setShowProblems] = useState(false);
  const [plan, setPlan] = useState<BindingPlan>();
  const [boundRef, setBoundRef] = useState<KeyRef>();

  if (!query.data) {
    if (query.error) return <FaultAlert error={query.error} title="Could not load backends" />;
    return <p className="text-muted-foreground text-sm">Loading backends...</p>;
  }

  const bindable = query.data.backends.filter((candidate) => candidate.lifecycle === KeyBackendLifecycle.ACTIVE);
  const backend = bindable.find((candidate) => candidate.id === (chosenBackend ?? bindable[0]?.id));
  const catalogue = query.data.catalogue;
  const purpose = chosenPurpose ?? catalogue[0]?.id;

  const backendOptions: readonly RadioCardOption<string>[] = bindable.map((candidate) => ({
    value: candidate.id,
    label: candidate.displayName,
    description: describeBackend(candidate),
  }));

  const locatorError = backend ? locatorProblem(backend.provider, locator) : undefined;
  const capabilityError = capabilities.length === 0 ? 'Request at least the envelope wrapping capability.' : undefined;

  const submitKey = async () => {
    setShowProblems(true);
    if (!backend || !purpose || locatorError || capabilityError) return;
    prepare.reset();
    try {
      setPlan(await prepare.mutateAsync({ backendId: backend.id, locator: locator.trim(), purpose, capabilities }));
      setStep(2);
    } catch {
      return;
    }
  };

  const toggleCapability = (capability: KeyCapability, on: boolean) =>
    setCapabilities((previous) => (on ? [...previous, capability] : previous.filter((value) => value !== capability)));

  return (
    <div className="flex flex-col gap-6">
      <FaultAlert error={query.error} title="Could not refresh backends" />
      <StepIndicator steps={steps} current={step} />

      {step === 0 ? (
        <Card>
          <CardHeader>
            <div className="flex flex-col gap-1">
              <CardTitle>Choose a backend</CardTitle>
              <CardDescription>A key is bound inside a backend you have already registered.</CardDescription>
            </div>
          </CardHeader>
          <CardContent className="flex flex-col gap-4">
            {bindable.length === 0 ? (
              <p className="text-muted-foreground text-[13px]">
                No backend is registered yet.{' '}
                <Link to="/keys/backends/new" className="text-primary-ink underline">
                  Register a backend
                </Link>{' '}
                first.
              </p>
            ) : (
              <RadioCards
                name="bind-backend"
                label="Backend"
                value={backend?.id ?? ''}
                options={backendOptions}
                onChange={setChosenBackend}
              />
            )}
            <div className="flex justify-end">
              <Button type="button" disabled={!backend} onClick={() => setStep(1)}>
                Continue
                <ArrowRightIcon data-icon="inline-end" />
              </Button>
            </div>
          </CardContent>
        </Card>
      ) : null}

      {step === 1 && backend ? (
        <form
          noValidate
          aria-label="Key details"
          onSubmit={(event) => {
            event.preventDefault();
            void submitKey();
          }}
        >
          <Card>
            <CardHeader>
              <div className="flex flex-col gap-1">
                <CardTitle>Key in {backend.displayName}</CardTitle>
                <CardDescription>
                  The platform binds exactly the key you name and returns an opaque reference. Only you can read the
                  locator back afterwards.
                </CardDescription>
              </div>
            </CardHeader>
            <CardContent className="flex flex-col gap-5">
              <TextField
                id="key-locator"
                label={`${keyProviderLabels[backend.provider]} key locator`}
                mono
                value={locator}
                onChange={setLocator}
                placeholder={locatorPlaceholders[backend.provider]}
                hint={locatorHints[backend.provider]}
                error={showProblems ? locatorError : undefined}
              />
              {catalogue.length === 0 ? (
                <p className="text-muted-foreground text-[13px]">No key purposes are available.</p>
              ) : (
                <RadioCards
                  name="key-purpose"
                  label="Purpose"
                  value={purpose ?? ''}
                  options={catalogue.map((entry) => ({
                    value: entry.id as string,
                    label: entry.label,
                    description: `${entry.description} Owned by the ${entry.owningService}.`,
                  }))}
                  onChange={(value) => setChosenPurpose(catalogue.find((entry) => entry.id === value)?.id)}
                  columns={2}
                  hint="A key serves one purpose. Binding it does not make it the default."
                />
              )}
              <fieldset className="flex flex-col gap-2">
                <legend className="micro text-muted-foreground mb-1.5">Capabilities</legend>
                {requestableCapabilities.map((capability) => (
                  <div key={capability} className="flex items-start gap-2">
                    <input
                      id={`capability-${capability}`}
                      type="checkbox"
                      className="mt-0.5 size-4"
                      checked={capabilities.includes(capability)}
                      onChange={(event) => toggleCapability(capability, event.target.checked)}
                    />
                    <div className="flex flex-col gap-0.5">
                      <Label htmlFor={`capability-${capability}`}>{keyCapabilityLabels[capability]}</Label>
                      <span className="text-muted-foreground text-xs">{keyCapabilityDescriptions[capability]}</span>
                    </div>
                  </div>
                ))}
                {showProblems && capabilityError ? <FieldError>{capabilityError}</FieldError> : null}
              </fieldset>
              <FaultAlert error={prepare.error} title="The key could not be prepared" />
              <div className="flex flex-wrap items-center justify-between gap-2">
                <Button type="button" variant="outline" onClick={() => setStep(0)}>
                  <ArrowLeftIcon data-icon="inline-start" />
                  Back
                </Button>
                <Button type="submit" disabled={!purpose || prepare.isPending}>
                  {prepare.isPending ? 'Preparing...' : 'Continue to validation'}
                  <ArrowRightIcon data-icon="inline-end" />
                </Button>
              </div>
            </CardContent>
          </Card>
        </form>
      ) : null}

      {step === 2 && plan ? (
        <ValidateStep
          key={plan.attemptId}
          plan={plan}
          onBack={() => setStep(1)}
          onDone={(ref) => {
            setBoundRef(ref);
            setStep(3);
          }}
        />
      ) : null}

      {step === 3 && boundRef ? (
        <Card>
          <CardContent className="flex flex-col items-start gap-4">
            <div className="flex items-center gap-2">
              <CheckCircle2Icon className="text-success-ink size-5" aria-hidden="true" />
              <h2 className="text-lg font-semibold">Key bound</h2>
            </div>
            <div className="flex flex-col gap-1.5">
              <span className="micro text-muted-foreground text-[10px]">Key reference</span>
              <span className="flex items-center gap-2">
                <code className="bg-surface-deep border-border rounded-md border px-2 py-1 font-mono text-sm">
                  {boundRef}
                </code>
                <CopyButton value={boundRef} label="key reference" />
              </span>
            </div>
            <p className="text-muted-foreground max-w-xl text-[13px]">
              The key is bound but not yet used. Make it the default for its purpose to wrap new payloads, or change
              custody to move existing ones.
            </p>
            <div className="flex flex-wrap items-center gap-2">
              <Link to="/keys/keys/$keyRef" params={{ keyRef: boundRef }} className={buttonVariants({ size: 'lg' })}>
                View key
              </Link>
              <Link to="/keys/migrations" className={buttonVariants({ variant: 'outline', size: 'lg' })}>
                Migrations
              </Link>
            </div>
          </CardContent>
        </Card>
      ) : null}
    </div>
  );
}
