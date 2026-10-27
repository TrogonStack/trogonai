import type { SecretPurpose } from '@trogonai/proto';

import { FieldShell, SegmentedField } from '@/components/field';
import { Input } from '@/components/ui/input';
import type { SegmentedOption } from '@/components/ui/segmented';

import { selectableVerificationOutcomes, verificationOutcomeLabels } from '../domain/labels';
import { admitsGeneration, requiresExternalVerification } from '../domain/policy';
import { PENDING_SIMULATION, effectiveOrigin, type MaterialDraft, type MaterialOrigin } from '../forms';

const originOptions: readonly SegmentedOption<MaterialOrigin>[] = [
  { value: 'imported', label: 'Import' },
  { value: 'generated', label: 'Generate' },
];

const simulationOptions: readonly SegmentedOption<string>[] = [
  ...selectableVerificationOutcomes.map((outcome) => ({
    value: String(outcome),
    label: verificationOutcomeLabels[outcome],
  })),
  { value: PENDING_SIMULATION, label: 'Leave pending' },
];

interface MaterialFieldsProps {
  idPrefix: string;
  purpose: SecretPurpose;
  value: MaterialDraft;
  onChange: (value: MaterialDraft) => void;
  error?: string;
  allowGenerate?: boolean;
}

export function MaterialFields({
  idPrefix,
  purpose,
  value,
  onChange,
  error,
  allowGenerate = true,
}: MaterialFieldsProps) {
  const generation = allowGenerate && admitsGeneration(purpose);
  const origin = effectiveOrigin(value, purpose);
  const verification = requiresExternalVerification(purpose);
  const id = `${idPrefix}-material`;

  return (
    <div className="flex flex-col gap-4">
      {generation ? (
        <SegmentedField
          id={`${idPrefix}-origin`}
          label="Material origin"
          value={origin}
          options={originOptions}
          onChange={(next) => onChange({ ...value, origin: next })}
          hint={
            origin === 'generated'
              ? 'A high-entropy secret is generated for you and shown exactly once.'
              : 'You supply the secret. It is write-only and never shown again.'
          }
        />
      ) : null}
      {origin === 'imported' ? (
        <FieldShell
          id={id}
          label="Secret material"
          error={error}
          hint="Write-only. The value is never stored here, never echoed, and never shown again."
        >
          <Input
            id={id}
            name={id}
            type="password"
            autoComplete="new-password"
            spellCheck={false}
            aria-invalid={error ? true : undefined}
            className="h-8 font-mono text-xs md:text-xs"
            value={value.material}
            onChange={(event) => onChange({ ...value, material: event.target.value })}
          />
        </FieldShell>
      ) : null}
      {verification ? (
        <fieldset className="border-border bg-surface-deep/40 flex min-w-0 flex-col gap-2 rounded-md border border-dashed p-3">
          <legend className="micro text-muted-foreground px-1 text-[10px]">Simulation</legend>
          <SegmentedField
            id={`${idPrefix}-simulation`}
            label="Verification outcome"
            value={value.simulation}
            options={simulationOptions}
            onChange={(next) => onChange({ ...value, simulation: next })}
            hint="No real provider is called. Choose how the fake backend verifies the candidate. Leave it pending to complete verification later from the credential."
          />
        </fieldset>
      ) : (
        <p className="text-muted-foreground text-xs">
          {admitsGeneration(purpose)
            ? 'Webhook secrets get local admission checks only. No external verification runs.'
            : 'This purpose is activated after local admission checks only.'}
        </p>
      )}
    </div>
  );
}
