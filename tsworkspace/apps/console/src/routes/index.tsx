import { createFileRoute } from '@tanstack/react-router';

import { HomeScreen } from '@/screens/home-screen';

export const Route = createFileRoute('/')({ component: HomeScreen });
