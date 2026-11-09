import { EyeIcon, EyeOffIcon, ShieldAlertIcon } from 'lucide-react';
import { useState } from 'react';

import { CopyButton } from '@/components/copy-button';
import { Button } from '@/components/ui/button';

interface SecretRevealProps {
  value: string;
  version: number;
  onAcknowledge: () => void;
}

export function SecretReveal({ value, version, onAcknowledge }: SecretRevealProps) {
  const [shown, setShown] = useState(false);
  return (
    <section
      aria-label="Generated secret"
      className="border-warning/35 bg-warning-soft flex flex-col gap-3 rounded-lg border p-4"
    >
      <div className="flex items-start gap-2.5">
        <ShieldAlertIcon className="text-warning-ink mt-0.5 size-4 shrink-0" aria-hidden="true" />
        <div className="flex flex-col gap-1">
          <p className="text-warning-ink text-sm font-medium">Copy this secret now. It is shown once.</p>
          <p className="text-warning-ink/90 text-[13px]">
            Version {version} was generated for you. The console never stores it and cannot show it again. Leaving this
            panel discards it for good.
          </p>
        </div>
      </div>
      <div className="well border-border flex items-center gap-2 rounded-md border px-3 py-2">
        <code data-testid="revealed-secret" className="min-w-0 flex-1 font-mono text-xs break-all select-all">
          {shown ? value : '•'.repeat(Math.min(value.length, 36))}
        </code>
        <Button
          type="button"
          variant="outline"
          size="xs"
          aria-pressed={shown}
          onClick={() => setShown((current) => !current)}
        >
          {shown ? <EyeOffIcon data-icon="inline-start" /> : <EyeIcon data-icon="inline-start" />}
          {shown ? 'Hide' : 'Show'}
        </Button>
        <CopyButton value={value} label="generated secret" />
      </div>
      <div className="flex justify-end">
        <Button type="button" onClick={onAcknowledge}>
          I have stored it
        </Button>
      </div>
    </section>
  );
}
