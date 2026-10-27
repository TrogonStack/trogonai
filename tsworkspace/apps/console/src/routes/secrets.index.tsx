import { createFileRoute } from '@tanstack/react-router';

import { SecretsScreen } from '@/screens/secrets-screen';

export const Route = createFileRoute('/secrets/')({ component: SecretsScreen });
