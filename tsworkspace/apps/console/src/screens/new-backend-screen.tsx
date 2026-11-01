import { BackLink } from '@/components/back-link';
import { PageHeader } from '@/components/page-header';
import { RegisterBackendWizard } from '@/features/custody/components/register-backend-wizard';

export function NewBackendScreen() {
  return (
    <div className="flex flex-col gap-6">
      <div>
        <BackLink to="/keys/backends">All backends</BackLink>
      </div>
      <PageHeader
        eyebrow="Security"
        title="Register a key backend"
        description="Grant the platform scoped, revocable access to a key provider you control. The platform never stores a long-lived secret for it."
      />
      <RegisterBackendWizard />
    </div>
  );
}
