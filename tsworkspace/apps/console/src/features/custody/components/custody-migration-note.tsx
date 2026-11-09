import { InfoIcon } from 'lucide-react';

export function CustodyMigrationNote() {
  return (
    <div
      role="note"
      className="border-info/30 bg-info-soft text-info-ink flex items-start gap-2.5 rounded-lg border px-4 py-3 text-[13px]"
    >
      <InfoIcon className="mt-0.5 size-4 shrink-0" aria-hidden="true" />
      <p>
        This is a custody migration, not a response to compromise. It rewraps each data key under the new key and leaves
        the encrypted data untouched. If a key may have been exposed, rewrapping is not enough: the data keys must be
        replaced and the data re-encrypted.
      </p>
    </div>
  );
}
