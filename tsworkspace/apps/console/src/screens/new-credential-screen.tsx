import { useNavigate } from '@tanstack/react-router';
import { useState } from 'react';

import { FaultAlert } from '@/components/fault-alert';
import { PageHeader } from '@/components/page-header';
import { Skeleton } from '@/components/ui/skeleton';
import { useNotify } from '@/components/ui/toaster';
import type { RevealedSecret } from '@/features/secrets/client';
import { CredentialForm } from '@/features/secrets/components/credential-form';
import { SecretReveal } from '@/features/secrets/components/secret-reveal';
import { useCreateCredential, useVaultsQuery, type CreateCredentialInput } from '@/features/secrets/queries';

export function NewCredentialScreen({ vaultId }: { vaultId?: string }) {
  const navigate = useNavigate();
  const vaults = useVaultsQuery();
  const create = useCreateCredential();
  const notify = useNotify();
  const [revealed, setRevealed] = useState<RevealedSecret>();

  const open = (secret: string) => navigate({ to: '/secrets/credentials/$secretRef', params: { secretRef: secret } });

  const submit = async (input: CreateCredentialInput) => {
    create.reset();
    try {
      const result = await create.mutateAsync(input);
      notify.success('Credential created', result.credential.displayName);
      if (result.revealed) {
        setRevealed(result.revealed);
        return;
      }
      await open(result.credential.secret);
    } catch {
      return;
    }
  };

  return (
    <div className="flex flex-col gap-6">
      <PageHeader
        eyebrow="Secret management"
        title="Add credential"
        description="Policy is committed at creation and cannot be edited afterwards. Material is write-only."
      />
      {revealed ? (
        <SecretReveal
          value={revealed.value}
          version={revealed.version}
          onAcknowledge={() => {
            const secret = revealed.secret;
            setRevealed(undefined);
            void open(secret);
          }}
        />
      ) : vaults.isError ? (
        <FaultAlert error={vaults.error} title="Could not load vaults" />
      ) : vaults.data ? (
        <CredentialForm
          vaults={vaults.data}
          initialVaultId={vaultId}
          pending={create.isPending}
          submitError={create.error}
          onSubmit={(input) => void submit(input)}
        />
      ) : (
        <Skeleton role="status" aria-label="Loading form" className="h-96 w-full" />
      )}
    </div>
  );
}
