import { useState } from 'react';

export function useOnOpen(open: boolean, restore: () => void) {
  const [wasOpen, setWasOpen] = useState(open);
  if (open !== wasOpen) {
    setWasOpen(open);
    if (open) restore();
  }
}
