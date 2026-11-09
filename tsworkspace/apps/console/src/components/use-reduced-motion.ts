import { useSyncExternalStore } from 'react';

const query = '(prefers-reduced-motion: reduce)';

function subscribe(onChange: () => void) {
  if (typeof window.matchMedia !== 'function') return () => {};
  const media = window.matchMedia(query);
  media.addEventListener('change', onChange);
  return () => media.removeEventListener('change', onChange);
}

function snapshot() {
  return typeof window.matchMedia === 'function' ? window.matchMedia(query).matches : true;
}

export function usePrefersReducedMotion(): boolean {
  return useSyncExternalStore(subscribe, snapshot, () => true);
}
