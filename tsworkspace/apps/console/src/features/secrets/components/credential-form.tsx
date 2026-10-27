import { Link } from '@tanstack/react-router';
import { SecretMaterialKind, SecretPurpose, VaultLifecycle } from '@trogonai/proto';
import { useMemo, useState, type ReactNode } from 'react';

import { FaultAlert } from '@/components/fault-alert';
import { RadioCards, SelectField, TextField, type RadioCardOption } from '@/components/field';
import { buttonVariants, Button } from '@/components/ui/button';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';

import { kindLabels, purposeDescriptions, purposeLabels } from '../domain/labels';
import { compatibleKinds, defaultKind, selectablePurposes } from '../domain/policy';
import type { VaultView } from '../domain/view';
import { initialMaterialDraft, materialError, toMaterialInput, toSimulation, type MaterialDraft } from '../forms';
import type { CreateCredentialInput } from '../queries';
import { EnvironmentChip } from './badges';
import { MaterialFields } from './material-fields';

const purposeOptions: readonly RadioCardOption<number>[] = selectablePurposes.map((purpose) => ({
  value: purpose as number,
  label: purposeLabels[purpose],
  description: purposeDescriptions[purpose],
}));

function ReadOnly({ label, hint, children }: { label: string; hint: string; children: ReactNode }) {
  return (
    <div className="flex min-w-0 flex-col gap-1.5">
      <span className="micro text-muted-foreground">{label}</span>
      <div className="flex h-8 items-center">{children}</div>
      <p className="text-muted-foreground text-xs">{hint}</p>
    </div>
  );
}

interface CredentialFormProps {
  vaults: readonly VaultView[];
  initialVaultId?: string;
  pending: boolean;
  submitError: unknown;
  onSubmit: (input: CreateCredentialInput) => void;
}

export function CredentialForm({ vaults, initialVaultId, pending, submitError, onSubmit }: CredentialFormProps) {
  const openVaults = useMemo(() => vaults.filter((vault) => vault.lifecycle === VaultLifecycle.ACTIVE), [vaults]);
  const [vaultId, setVaultId] = useState(() =>
    openVaults.some((vault) => vault.vaultId === initialVaultId) ? initialVaultId! : (openVaults[0]?.vaultId ?? ''),
  );
  const [displayName, setDisplayName] = useState('');
  const [purpose, setPurpose] = useState<SecretPurpose>(SecretPurpose.PROVIDER_AUTHENTICATION);
  const [kind, setKind] = useState<SecretMaterialKind>(defaultKind(SecretPurpose.PROVIDER_AUTHENTICATION));
  const [material, setMaterial] = useState<MaterialDraft>(initialMaterialDraft);
  const [attempted, setAttempted] = useState(false);

  const vault = openVaults.find((candidate) => candidate.vaultId === vaultId);
  const kinds = compatibleKinds(purpose);
  const nameError = attempted && displayName.trim() === '' ? 'A display name is required.' : undefined;
  const vaultError = attempted && !vault ? 'Choose an active vault.' : undefined;
  const secretError = attempted ? materialError(material, purpose, kind) : undefined;

  const choosePurpose = (next: SecretPurpose) => {
    setPurpose(next);
    setKind(defaultKind(next));
    setMaterial((current) => ({ ...current, origin: 'imported', material: '' }));
  };

  const submit = () => {
    setAttempted(true);
    if (!vault || displayName.trim() === '' || materialError(material, purpose, kind)) return;
    onSubmit({
      vaultId: vault.vaultId,
      displayName,
      purpose,
      kind,
      material: toMaterialInput(material, purpose),
      simulation: toSimulation(material, purpose),
    });
  };

  if (openVaults.length === 0) {
    return (
      <div className="border-border bg-card flex flex-col items-start gap-3 rounded-lg border p-6">
        <p className="text-sm font-medium">No active vault</p>
        <p className="text-muted-foreground max-w-md text-[13px]">
          A credential lives in an active vault. Create or enable a vault first.
        </p>
        <Link to="/secrets" className={buttonVariants({ variant: 'outline', size: 'sm' })}>
          Back to secrets
        </Link>
      </div>
    );
  }

  return (
    <form
      noValidate
      aria-label="Add credential"
      className="flex max-w-3xl flex-col gap-4"
      onSubmit={(event) => {
        event.preventDefault();
        submit();
      }}
    >
      <FaultAlert error={submitError} title="The credential was rejected" />
      <Card>
        <CardHeader>
          <CardTitle>Placement</CardTitle>
        </CardHeader>
        <CardContent className="grid grid-cols-1 gap-3 md:grid-cols-2">
          <SelectField
            id="credential-vault"
            label="Vault"
            value={vaultId}
            options={openVaults.map((candidate) => ({ value: candidate.vaultId, label: candidate.displayName }))}
            onChange={setVaultId}
            error={vaultError}
          />
          <ReadOnly label="Environment" hint="Inherited from the vault.">
            {vault ? <EnvironmentChip environment={vault.environment} /> : null}
          </ReadOnly>
          <TextField
            id="credential-name"
            label="Display name"
            value={displayName}
            onChange={setDisplayName}
            error={nameError}
            placeholder="OpenAI production key"
            className="md:col-span-2"
          />
        </CardContent>
      </Card>
      <Card>
        <CardHeader>
          <CardTitle>Purpose</CardTitle>
        </CardHeader>
        <CardContent className="flex flex-col gap-3">
          <RadioCards
            name="credential-purpose"
            label="What the credential is for"
            value={purpose as number}
            options={purposeOptions}
            columns={2}
            onChange={(next) => choosePurpose(next as SecretPurpose)}
            hint="The purpose is immutable once the credential exists."
          />
          {kinds.length > 1 ? (
            <SelectField
              id="credential-kind"
              label="Material kind"
              value={kind as number}
              options={kinds.map((candidate) => ({ value: candidate as number, label: kindLabels[candidate] }))}
              onChange={(next) => setKind(next as SecretMaterialKind)}
              hint="The kind is immutable once the credential exists."
            />
          ) : (
            <ReadOnly label="Material kind" hint="Fixed by the purpose.">
              <span className="text-[13px]">{kindLabels[kind]}</span>
            </ReadOnly>
          )}
        </CardContent>
      </Card>
      <Card>
        <CardHeader>
          <CardTitle>Material</CardTitle>
        </CardHeader>
        <CardContent>
          <MaterialFields
            idPrefix="credential"
            purpose={purpose}
            value={material}
            onChange={setMaterial}
            error={secretError}
          />
        </CardContent>
      </Card>
      <div className="flex items-center gap-2">
        <Button type="submit" size="lg" disabled={pending}>
          {pending ? 'Creating...' : 'Create credential'}
        </Button>
        <Link to="/secrets" className={buttonVariants({ variant: 'ghost', size: 'lg' })}>
          Cancel
        </Link>
      </div>
    </form>
  );
}
