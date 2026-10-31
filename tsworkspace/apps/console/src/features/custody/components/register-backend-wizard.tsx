import { Link } from '@tanstack/react-router';
import { KeyProvider } from '@trogonai/proto';
import { ArrowLeftIcon, ArrowRightIcon, CheckCircle2Icon } from 'lucide-react';
import { useState } from 'react';

import { FaultAlert } from '@/components/fault-alert';
import {
  FieldShell,
  RadioCards,
  SegmentedField,
  SelectField,
  TextField,
  type RadioCardOption,
} from '@/components/field';
import { Button, buttonVariants } from '@/components/ui/button';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import { Textarea } from '@/components/ui/textarea';
import { useNotify } from '@/components/ui/toaster';

import { keyBackendIdOf, type KeyBackendId } from '../domain/identifiers';
import { keyProviderLabels, type RegistrableProvider } from '../domain/provider';
import { awsRegions, MIN_OPENBAO_VERSION, registrationProblems, type FieldProblems } from '../domain/registration';
import { emptyRegistrationDraft, registrationInputOf, type RegistrationDraft } from '../domain/registration-draft';
import { registrationChecks, type RegistrationPlan } from '../domain/validation';
import { useCompleteRegistration, usePrepareRegistration } from '../queries';
import { SetupInstructionsPanel } from './setup-instructions-panel';
import { StepIndicator } from './step-indicator';
import { useStagedChecks, ValidationChecklist, type StagedChecks } from './validation-checklist';

const steps = ['Provider', 'Details', 'Set up access', 'Validate', 'Done'] as const;

const providerOptions: readonly RadioCardOption<RegistrableProvider>[] = [
  {
    value: KeyProvider.AWS_KMS,
    label: keyProviderLabels[KeyProvider.AWS_KMS],
    description: 'The platform assumes an IAM role in your account, proven by a tenant-unique external ID.',
  },
  {
    value: KeyProvider.GOOGLE_CLOUD_KMS,
    label: keyProviderLabels[KeyProvider.GOOGLE_CLOUD_KMS],
    description: 'The platform federates into your project through Workload Identity Federation.',
  },
  {
    value: KeyProvider.OPENBAO_TRANSIT,
    label: keyProviderLabels[KeyProvider.OPENBAO_TRANSIT],
    description: `A Transit mount you operate. The platform reaches it over verified HTTPS. Version ${MIN_OPENBAO_VERSION} or newer.`,
  },
];

function ProviderFields({
  provider,
  draft,
  problems,
  set,
}: {
  provider: RegistrableProvider;
  draft: RegistrationDraft;
  problems: FieldProblems;
  set: <K extends keyof RegistrationDraft>(key: K, value: RegistrationDraft[K]) => void;
}) {
  return (
    <div className="grid gap-4 md:grid-cols-2">
      <TextField
        id="backend-name"
        label="Display name"
        value={draft.displayName}
        onChange={(value) => set('displayName', value)}
        error={problems.displayName}
        hint="Shown wherever the backend is listed."
        className="md:col-span-2"
      />
      {provider === KeyProvider.AWS_KMS ? (
        <>
          <SelectField
            id="aws-region"
            label="Region"
            value={draft.region}
            options={awsRegions.map((region) => ({ value: region, label: region }))}
            onChange={(value) => set('region', value)}
            hint="Where your keys live."
          />
          <TextField
            id="aws-role-arn"
            label="Role ARN"
            mono
            value={draft.roleArn}
            onChange={(value) => set('roleArn', value)}
            placeholder="arn:aws:iam::512049177301:role/trogon-key-access"
            error={problems.roleArn}
            hint="The role the platform assumes. You create it in the next step."
          />
        </>
      ) : null}
      {provider === KeyProvider.GOOGLE_CLOUD_KMS ? (
        <>
          <TextField
            id="gcp-project"
            label="Project id"
            mono
            value={draft.projectId}
            onChange={(value) => set('projectId', value)}
            placeholder="acme-prod-kms"
            error={problems.projectId}
          />
          <TextField
            id="gcp-location"
            label="Location"
            mono
            value={draft.location}
            onChange={(value) => set('location', value)}
            placeholder="europe-west1"
            error={problems.location}
            hint="Where your keys live."
          />
          <TextField
            id="gcp-service-account"
            label="Impersonated service account (optional)"
            mono
            value={draft.impersonatedServiceAccount}
            onChange={(value) => set('impersonatedServiceAccount', value)}
            placeholder="trogon-kms@acme-prod-kms.iam.gserviceaccount.com"
            error={problems.impersonatedServiceAccount}
            hint="Leave empty to grant the federated principal direct access."
          />
          <TextField
            id="gcp-provider"
            label="Workload identity provider"
            mono
            value={draft.workloadIdentityProvider}
            onChange={(value) => set('workloadIdentityProvider', value)}
            placeholder="//iam.googleapis.com/projects/482913075521/locations/global/workloadIdentityPools/trogon/providers/trogon"
            error={problems.workloadIdentityProvider}
            className="md:col-span-2"
          />
        </>
      ) : null}
      {provider === KeyProvider.OPENBAO_TRANSIT ? (
        <>
          <TextField
            id="bao-endpoint"
            label="Endpoint"
            mono
            value={draft.endpoint}
            onChange={(value) => set('endpoint', value)}
            placeholder="https://bao.acme.example:8200"
            error={problems.endpoint}
            hint="HTTPS only. Loopback, link-local and undeclared internal addresses are rejected."
            className="md:col-span-2"
          />
          <TextField
            id="bao-namespace"
            label="Namespace"
            mono
            value={draft.namespace}
            onChange={(value) => set('namespace', value)}
            placeholder="team-a"
            error={problems.namespace}
            hint="Leave empty for the root namespace. Changing it later needs a new registration."
          />
          <TextField
            id="bao-mount"
            label="Transit mount"
            mono
            value={draft.transitMount}
            onChange={(value) => set('transitMount', value)}
            error={problems.transitMount}
          />
          <TextField
            id="bao-role"
            label="Auth role"
            mono
            value={draft.authRole}
            onChange={(value) => set('authRole', value)}
            placeholder="trogon-transit"
            error={problems.authRole}
            hint="The role the platform logs in with."
          />
          <TextField
            id="bao-version"
            label="OpenBao version"
            mono
            value={draft.version}
            onChange={(value) => set('version', value)}
            placeholder="2.6.1"
            error={problems.version}
            hint={`Version ${MIN_OPENBAO_VERSION} or newer.`}
          />
          <SegmentedField
            id="bao-auth"
            label="Authentication"
            value={draft.authMethod}
            options={[
              { value: 'jwt', label: 'JWT' },
              { value: 'certificate', label: 'Certificate' },
            ]}
            onChange={(value) => set('authMethod', value)}
          />
          <FieldShell
            id="bao-trust-root"
            label="Trust root certificate"
            error={problems.trustRootPem}
            hint="PEM encoded. The platform verifies TLS against this explicit trust root and nothing else."
            className="md:col-span-2"
          >
            <Textarea
              id="bao-trust-root"
              name="bao-trust-root"
              value={draft.trustRootPem}
              onChange={(event) => set('trustRootPem', event.target.value)}
              placeholder="-----BEGIN CERTIFICATE-----"
              spellCheck={false}
              aria-invalid={problems.trustRootPem ? true : undefined}
              className="font-mono text-xs md:text-xs"
            />
          </FieldShell>
        </>
      ) : null}
    </div>
  );
}

function ValidateStep({
  plan,
  staged,
  onBack,
  onDone,
}: {
  plan: RegistrationPlan;
  staged: StagedChecks;
  onBack: () => void;
  onDone: (id: KeyBackendId) => void;
}) {
  const complete = useCompleteRegistration();
  const notify = useNotify();

  const finish = async () => {
    try {
      const registration = await complete.mutateAsync(plan.attemptId);
      notify.success('Backend registered', registration.displayName);
      onDone(keyBackendIdOf(registration.registration.keyBackendId));
    } catch {
      return;
    }
  };

  return (
    <div className="flex flex-col gap-4">
      <ValidationChecklist
        title="Validation"
        description="Each check runs against your backend in order. Nothing is retained until every check passes."
        checks={plan.checks}
        staged={staged}
      />
      {staged.failed ? (
        <p
          role="status"
          className="border-warning/35 bg-warning-soft text-warning-ink rounded-lg border px-4 py-3 text-[13px]"
        >
          Fix the failing item in your backend, then run the checks again. Checks that already passed are not repeated.
        </p>
      ) : null}
      <FaultAlert error={complete.error} title="The registration was rejected" />
      <div className="flex flex-wrap items-center justify-between gap-2">
        <Button type="button" variant="outline" onClick={onBack} disabled={staged.running}>
          <ArrowLeftIcon data-icon="inline-start" />
          Back to instructions
        </Button>
        <Button type="button" disabled={!staged.allPassed || complete.isPending} onClick={() => void finish()}>
          {complete.isPending ? 'Registering...' : 'Register backend'}
        </Button>
      </div>
    </div>
  );
}

export function RegisterBackendWizard() {
  const prepare = usePrepareRegistration();
  const [step, setStep] = useState(0);
  const [provider, setProvider] = useState<RegistrableProvider>(KeyProvider.AWS_KMS);
  const [draft, setDraft] = useState<RegistrationDraft>(emptyRegistrationDraft);
  const [showProblems, setShowProblems] = useState(false);
  const [plan, setPlan] = useState<RegistrationPlan>();
  const staged = useStagedChecks(plan?.attemptId, plan?.checks ?? []);
  const [registeredId, setRegisteredId] = useState<KeyBackendId>();

  const set = <K extends keyof RegistrationDraft>(key: K, value: RegistrationDraft[K]) =>
    setDraft((previous) => ({ ...previous, [key]: value }));

  const input = registrationInputOf(provider, draft);
  const problems = registrationProblems(input);
  const hasProblems = Object.keys(problems).length > 0;

  const submitDetails = async () => {
    setShowProblems(true);
    if (hasProblems) return;
    if (plan && JSON.stringify(plan.input) === JSON.stringify(input)) {
      setStep(2);
      return;
    }
    prepare.reset();
    try {
      setPlan(await prepare.mutateAsync(input));
      setStep(2);
    } catch {
      return;
    }
  };

  const previewChecks = registrationChecks(provider, draft.impersonatedServiceAccount.trim() !== '');

  return (
    <div className="flex flex-col gap-6">
      <StepIndicator steps={steps} current={step} />

      {step === 0 ? (
        <Card>
          <CardHeader>
            <div className="flex flex-col gap-1">
              <CardTitle>Choose a provider</CardTitle>
              <CardDescription>
                A backend is the trust boundary the platform is allowed to reach. Keys are bound to it afterwards.
              </CardDescription>
            </div>
          </CardHeader>
          <CardContent className="flex flex-col gap-4">
            <RadioCards
              name="provider"
              label="Key provider"
              value={provider}
              options={providerOptions}
              onChange={setProvider}
              columns={3}
            />
            <div className="flex justify-end">
              <Button type="button" onClick={() => setStep(1)}>
                Continue
                <ArrowRightIcon data-icon="inline-end" />
              </Button>
            </div>
          </CardContent>
        </Card>
      ) : null}

      {step === 1 ? (
        <form
          noValidate
          aria-label="Backend details"
          onSubmit={(event) => {
            event.preventDefault();
            void submitDetails();
          }}
        >
          <Card>
            <CardHeader>
              <div className="flex flex-col gap-1">
                <CardTitle>{keyProviderLabels[provider]} details</CardTitle>
                <CardDescription>
                  A registration holds no key locator. It only describes how the platform authenticates.
                </CardDescription>
              </div>
            </CardHeader>
            <CardContent className="flex flex-col gap-4">
              <ProviderFields provider={provider} draft={draft} problems={showProblems ? problems : {}} set={set} />
              <FaultAlert error={prepare.error} title="The backend could not be prepared" />
              <div className="flex flex-wrap items-center justify-between gap-2">
                <Button type="button" variant="outline" onClick={() => setStep(0)}>
                  <ArrowLeftIcon data-icon="inline-start" />
                  Back
                </Button>
                <Button type="submit" disabled={prepare.isPending}>
                  {prepare.isPending ? 'Preparing...' : 'Generate setup instructions'}
                  <ArrowRightIcon data-icon="inline-end" />
                </Button>
              </div>
            </CardContent>
          </Card>
        </form>
      ) : null}

      {step === 2 && plan ? (
        <div className="flex flex-col gap-4">
          {plan.externalId ? (
            <Card>
              <CardHeader>
                <div className="flex flex-col gap-1">
                  <CardTitle>Your external ID</CardTitle>
                  <CardDescription>
                    Generated for your tenant. It is already in the trust policy below. Anyone without it cannot assume
                    the role.
                  </CardDescription>
                </div>
              </CardHeader>
              <CardContent>
                <code className="bg-surface-deep border-border rounded-md border px-2 py-1 font-mono text-xs">
                  {plan.externalId}
                </code>
              </CardContent>
            </Card>
          ) : null}
          <SetupInstructionsPanel instructions={plan.instructions} />
          <div className="flex flex-wrap items-center justify-between gap-2">
            <Button type="button" variant="outline" onClick={() => setStep(1)}>
              <ArrowLeftIcon data-icon="inline-start" />
              Back
            </Button>
            <Button type="button" onClick={() => setStep(3)}>
              I have applied these
              <ArrowRightIcon data-icon="inline-end" />
            </Button>
          </div>
        </div>
      ) : null}

      {step === 3 && plan ? (
        <ValidateStep
          key={plan.attemptId}
          plan={plan}
          staged={staged}
          onBack={() => setStep(2)}
          onDone={(id) => {
            setRegisteredId(id);
            setStep(4);
          }}
        />
      ) : null}

      {step === 4 && registeredId ? (
        <Card>
          <CardContent className="flex flex-col items-start gap-4">
            <div className="flex items-center gap-2">
              <CheckCircle2Icon className="text-success-ink size-5" aria-hidden="true" />
              <h2 className="text-lg font-semibold">Backend registered</h2>
            </div>
            <p className="text-muted-foreground max-w-xl text-[13px]">
              The platform can now reach {keyProviderLabels[provider]}. No key is bound yet, so nothing is encrypted
              under it.
            </p>
            <div className="flex flex-wrap items-center gap-2">
              <Link
                to="/keys/backends/$backendId"
                params={{ backendId: registeredId }}
                className={buttonVariants({ variant: 'outline', size: 'lg' })}
              >
                View backend
              </Link>
            </div>
          </CardContent>
        </Card>
      ) : null}

      {step === 0 || step === 1 ? (
        <p className="text-muted-foreground text-xs">
          Validation will run: {previewChecks.map((check) => check.label).join('; ')}.
        </p>
      ) : null}
    </div>
  );
}
