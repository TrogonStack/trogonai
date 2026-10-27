import { FieldShell, describedBy } from '@/components/field';
import { Textarea } from '@/components/ui/textarea';

import { MAX_DESCRIPTION_LENGTH } from '../domain/environment';

interface DescriptionFieldProps {
  id: string;
  value: string;
  onChange: (value: string) => void;
  error?: string;
}

export function DescriptionField({ id, value, onChange, error }: DescriptionFieldProps) {
  const hint = `Optional. Up to ${MAX_DESCRIPTION_LENGTH} characters.`;
  return (
    <FieldShell id={id} label="Description" error={error} hint={hint}>
      <Textarea
        id={id}
        name={id}
        value={value}
        rows={2}
        aria-invalid={error ? true : undefined}
        aria-describedby={describedBy(id, error, hint)}
        onChange={(event) => onChange(event.target.value)}
      />
    </FieldShell>
  );
}
