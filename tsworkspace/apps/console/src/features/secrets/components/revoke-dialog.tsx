import { CredentialPolicyReason } from '@trogonai/proto';
import { ShieldAlertIcon } from 'lucide-react';
import { useState } from 'react';

import { ConfirmDialog } from '@/components/confirm-dialog';
import { SelectField } from '@/components/field';
import { useNotify } from '@/components/ui/toaster';

import { policyReasonLabels, selectableRevocationReasons } from '../domain/labels';
import type { CredentialView } from '../domain/view';
import { useRevokeCredential } from '../queries';

const reasonOptions = selectableRevocationReasons.map((reason) => ({
  value: reason as number,
  label: policyReasonLabels[reason],
}));

interface RevokeDialogProps {
  credential: CredentialView;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

export function RevokeDialog({ credential, open, onOpenChange }: RevokeDialogProps) {
  const revoke = useRevokeCredential();
  const notify = useNotify();
  const [reason, setReason] = useState<CredentialPolicyReason>(CredentialPolicyReason.OPERATOR_REQUESTED);

  const close = (next: boolean) => {
    if (!next) {
      revoke.reset();
      setReason(CredentialPolicyReason.OPERATOR_REQUESTED);
    }
    onOpenChange(next);
  };

  return (
    <ConfirmDialog
      open={open}
      onOpenChange={close}
      icon={<ShieldAlertIcon />}
      title={`Revoke ${credential.displayName}?`}
      description="Revocation is irreversible. The credential stops serving every use immediately, any open webhook overlap ends, and it can never be enabled again. Only its destruction can follow."
      confirmLabel="Revoke credential"
      onConfirm={() => {
        revoke.mutate(
          { secret: credential.secret, expectedRevision: credential.revision, reason },
          {
            onSuccess: () => notify.success('Credential revoked', credential.displayName),
            onError: (error) => notify.failure('Revoke failed', error.message),
          },
        );
        close(false);
      }}
    >
      <SelectField
        id="revoke-reason"
        label="Reason"
        value={reason as number}
        options={reasonOptions}
        onChange={(next) => setReason(next as CredentialPolicyReason)}
        hint={
          reason === CredentialPolicyReason.COMPROMISE
            ? 'Compromise is recorded in the audit trail. Rotate the secret at the provider as well, since the old value may be known.'
            : undefined
        }
      />
    </ConfirmDialog>
  );
}
