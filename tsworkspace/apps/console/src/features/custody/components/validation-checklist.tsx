import { CheckCircle2Icon, CircleDashedIcon, CircleXIcon, Loader2Icon, MinusCircleIcon, PlayIcon } from 'lucide-react';
import { useState } from 'react';

import { FaultAlert } from '@/components/fault-alert';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';

import type { AttemptId } from '../domain/identifiers';
import type { CheckDefinition, CheckResult } from '../domain/validation';
import { useRunCheck } from '../queries';
import { FailureReasonBadge } from './key-badges';

export function useStagedChecks(attemptId: AttemptId | undefined, checks: readonly CheckDefinition[]) {
  const runCheck = useRunCheck();
  const [attemptResults, setAttemptResults] = useState<Readonly<Record<string, Readonly<Record<string, CheckResult>>>>>(
    {},
  );
  const results = attemptResults[attemptId ?? ''] ?? {};
  const [active, setActive] = useState<string>();
  const [attemptErrors, setAttemptErrors] = useState<Readonly<Record<string, unknown>>>({});
  const error = attemptErrors[attemptId ?? ''];

  const allPassed = checks.every((check) => results[check.id]?.outcome === 'pass');
  const failed = checks.some((check) => results[check.id]?.outcome === 'fail');

  const run = async () => {
    if (!attemptId) return;
    setAttemptErrors((previous) => ({ ...previous, [attemptId]: undefined }));
    for (const check of checks.filter((check) => results[check.id]?.outcome !== 'pass')) {
      setActive(check.id);
      try {
        const result = await runCheck.mutateAsync({ attemptId, checkId: check.id });
        setAttemptResults((previous) => ({ ...previous, [attemptId]: { ...previous[attemptId], [check.id]: result } }));
        if (result.outcome !== 'pass') break;
      } catch (caught) {
        setAttemptErrors((previous) => ({ ...previous, [attemptId]: caught }));
        break;
      }
    }
    setActive(undefined);
  };

  return { results, active, error, allPassed, failed, running: active !== undefined, run };
}

export type StagedChecks = ReturnType<typeof useStagedChecks>;

function Marker({ result, running }: { result: CheckResult | undefined; running: boolean }) {
  if (running) return <Loader2Icon className="text-info-ink size-4 animate-spin" aria-label="Running" />;
  if (!result) return <CircleDashedIcon className="text-muted-foreground size-4" aria-label="Not run" />;
  if (result.outcome === 'pass') return <CheckCircle2Icon className="text-success-ink size-4" aria-label="Passed" />;
  if (result.outcome === 'fail') return <CircleXIcon className="text-danger-ink size-4" aria-label="Failed" />;
  return <MinusCircleIcon className="text-muted-foreground size-4" aria-label="Skipped" />;
}

export function ValidationChecklist({
  title,
  description,
  checks,
  staged,
}: {
  title: string;
  description: string;
  checks: readonly CheckDefinition[];
  staged: StagedChecks;
}) {
  const ran = Object.keys(staged.results).length > 0;
  return (
    <Card>
      <CardHeader>
        <div className="flex flex-col gap-1">
          <CardTitle>{title}</CardTitle>
          <CardDescription>{description}</CardDescription>
        </div>
        {staged.allPassed ? null : (
          <Button type="button" size="sm" disabled={staged.running} onClick={() => void staged.run()}>
            <PlayIcon data-icon="inline-start" />
            {staged.failed ? 'Run checks again' : ran ? 'Continue checks' : 'Run checks'}
          </Button>
        )}
      </CardHeader>
      <CardContent className="flex flex-col gap-3">
        {staged.error ? <FaultAlert error={staged.error} title="A check could not run" /> : null}
        <ol className="divide-border divide-y" aria-label="Validation checks">
          {checks.map((check) => {
            const result = staged.results[check.id];
            return (
              <li key={check.id} className="flex items-start gap-3 py-2.5 first:pt-0 last:pb-0">
                <span className="mt-0.5">
                  <Marker result={result} running={staged.active === check.id} />
                </span>
                <div className="flex min-w-0 flex-1 flex-col gap-1">
                  <span className="text-[13px] font-medium">{check.label}</span>
                  {result ? (
                    <span
                      role={result.outcome === 'fail' ? 'alert' : undefined}
                      className={
                        result.outcome === 'fail' ? 'text-danger-ink text-xs' : 'text-muted-foreground text-xs'
                      }
                    >
                      {result.detail}
                    </span>
                  ) : null}
                </div>
                {result?.reason !== undefined ? <FailureReasonBadge reason={result.reason} /> : null}
              </li>
            );
          })}
        </ol>
      </CardContent>
    </Card>
  );
}
