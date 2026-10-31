import { createFileRoute } from '@tanstack/react-router';

import { KeyListScreen } from '@/screens/key-list-screen';

export const Route = createFileRoute('/keys/keys/')({ component: KeyListScreen });
