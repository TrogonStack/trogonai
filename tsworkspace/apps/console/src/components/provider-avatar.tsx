import { CloudIcon, LayersIcon, ServerIcon } from 'lucide-react';
import type { CSSProperties, ReactNode } from 'react';

import { cn } from '@/lib/utils';

interface Appearance {
  readonly hue: number;
  readonly icon?: ReactNode;
  readonly monogram?: string;
}

const known: Readonly<Record<string, Appearance>> = {
  openai: { hue: 162, monogram: 'O' },
  anthropic: { hue: 45, monogram: 'A' },
  'google-gemini': { hue: 255, monogram: 'G' },
  bedrock: { hue: 70, icon: <LayersIcon /> },
  'aws-bedrock': { hue: 70, icon: <LayersIcon /> },
  'cloudflare-ai-gateway': { hue: 50, icon: <CloudIcon /> },
  ollama: { hue: 300, icon: <ServerIcon /> },
};

function hashHue(value: string): number {
  let hash = 0;
  for (const character of value) hash = (hash * 31 + character.codePointAt(0)!) % 360;
  return hash;
}

function appearance(providerId: string, name: string | undefined): Appearance {
  const match = known[providerId];
  if (match) return match;
  const source = (name || providerId).trim();
  return { hue: hashHue(providerId), monogram: (source[0] ?? '?').toUpperCase() };
}

const sizes = {
  sm: 'size-6 rounded-[5px] text-[11px] [&_svg]:size-3.5',
  md: 'size-8 rounded-md text-[13px] [&_svg]:size-4',
  lg: 'size-12 rounded-lg text-xl [&_svg]:size-6',
} as const;

interface ProviderAvatarProps {
  providerId: string;
  name?: string;
  size?: keyof typeof sizes;
  muted?: boolean;
  className?: string;
}

export function ProviderAvatar({ providerId, name, size = 'md', muted, className }: ProviderAvatarProps) {
  const { hue, icon, monogram } = appearance(providerId, name);
  return (
    <span
      aria-hidden="true"
      style={{ '--avatar-hue': hue } as CSSProperties}
      className={cn(
        'avatar-tint inline-flex shrink-0 items-center justify-center font-mono font-bold select-none',
        sizes[size],
        muted && 'opacity-50 grayscale',
        className,
      )}
    >
      {icon ?? monogram}
    </span>
  );
}
