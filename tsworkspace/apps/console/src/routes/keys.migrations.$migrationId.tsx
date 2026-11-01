import { createFileRoute } from '@tanstack/react-router';

import { MigrationDetailScreen } from '@/screens/migration-detail-screen';

export const Route = createFileRoute('/keys/migrations/$migrationId')({ component: MigrationRoute });

function MigrationRoute() {
  const { migrationId } = Route.useParams();
  return <MigrationDetailScreen migrationId={migrationId} />;
}
