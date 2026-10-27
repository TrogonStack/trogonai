import { useEffect, useState } from 'react';

import { usePrefersReducedMotion } from '@/components/use-reduced-motion';

export function CountUp({ value, durationMs = 700, pad = 2 }: { value: number; durationMs?: number; pad?: number }) {
  const reduced = usePrefersReducedMotion();
  const [shown, setShown] = useState(reduced ? value : 0);

  useEffect(() => {
    if (reduced) {
      setShown(value);
      return;
    }
    let frame = 0;
    const start = performance.now();
    const tick = (now: number) => {
      const progress = Math.min(1, (now - start) / durationMs);
      const eased = 1 - Math.pow(1 - progress, 3);
      setShown(Math.round(value * eased));
      if (progress < 1) frame = requestAnimationFrame(tick);
    };
    frame = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(frame);
  }, [value, durationMs, reduced]);

  return (
    <>
      <span className="sr-only">{value}</span>
      <span aria-hidden="true">{String(shown).padStart(pad, '0')}</span>
    </>
  );
}
