export function CountTag({ count }: { count: number | undefined }) {
  if (count === undefined) return null;
  return (
    <span className="border-border bg-surface-deep/70 rounded-sm border px-1.5 font-mono text-[10px] leading-4 tabular-nums">
      {count}
    </span>
  );
}
