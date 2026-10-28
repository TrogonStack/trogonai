import { useNavigate } from '@tanstack/react-router';

import { PageHeader } from '@/components/page-header';
import { useNotify } from '@/components/ui/toaster';
import { RegistrationForm } from '@/features/provider-connections/components/registration-form';
import type { RegistrationDraft } from '@/features/provider-connections/domain/draft';
import { useRegisterConnection } from '@/features/provider-connections/queries';

export function NewProviderScreen() {
  const navigate = useNavigate();
  const register = useRegisterConnection();
  const notify = useNotify();

  const submit = async (draft: RegistrationDraft) => {
    register.reset();
    try {
      const view = await register.mutateAsync(draft);
      notify.success('Connection registered', view.displayName || view.id);
      await navigate({ to: '/providers/$connectionId', params: { connectionId: view.id } });
    } catch {
      return;
    }
  };

  return (
    <div className="flex flex-col gap-6">
      <PageHeader
        eyebrow="Model access"
        title="New provider connection"
        description="The configuration is committed by digest at registration and cannot be edited afterwards."
      />
      <RegistrationForm onSubmit={submit} submitError={register.error} />
    </div>
  );
}
