import { useCallback, useEffect, useRef, useState } from 'react';

import { usePrefersReducedMotion } from '@/components/use-reduced-motion';
import { cn } from '@/lib/utils';

const glyphs = '01#%&*<>/+=~$';
const played = new Set<string>();

function scramble(text: string, revealed: number): string {
  let out = '';
  for (let index = 0; index < text.length; index += 1) {
    const character = text[index]!;
    out +=
      index < revealed || /[^a-zA-Z0-9]/.test(character)
        ? character
        : glyphs[Math.floor(Math.random() * glyphs.length)];
  }
  return out;
}

export function ScrambleText({
  text,
  className,
  onHover = true,
}: {
  text: string;
  className?: string;
  onHover?: boolean;
}) {
  const reduced = usePrefersReducedMotion();
  const [display, setDisplay] = useState(text);
  const frame = useRef(0);

  const play = useCallback(() => {
    if (reduced) return;
    cancelAnimationFrame(frame.current);
    const start = performance.now();
    const duration = Math.min(700, 220 + text.length * 22);
    const tick = (now: number) => {
      const progress = Math.min(1, (now - start) / duration);
      setDisplay(progress >= 1 ? text : scramble(text, Math.floor(progress * text.length)));
      if (progress < 1) frame.current = requestAnimationFrame(tick);
    };
    frame.current = requestAnimationFrame(tick);
  }, [reduced, text]);

  useEffect(() => {
    setDisplay(text);
    if (!played.has(text)) {
      played.add(text);
      play();
    }
    return () => cancelAnimationFrame(frame.current);
  }, [text, play]);

  return (
    <span className={cn('font-mono', className)} onMouseEnter={onHover ? play : undefined}>
      {display}
    </span>
  );
}
