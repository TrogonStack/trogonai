import { BackLink } from '@/components/back-link';
import { PageHeader } from '@/components/page-header';
import { BindKeyWizard } from '@/features/custody/components/bind-key-wizard';

export function NewKeyScreen({ backendId }: { backendId?: string }) {
  return (
    <div className="flex flex-col gap-6">
      <div>
        <BackLink to="/keys/keys">All keys</BackLink>
      </div>
      <PageHeader
        eyebrow="Security"
        title="Bind a key"
        description="Name the exact key in your provider. The platform proves it can use it, then issues an opaque reference that never changes."
      />
      <BindKeyWizard initialBackendId={backendId} />
    </div>
  );
}
