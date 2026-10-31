import { KeyProvider } from '@trogonai/proto';

import type { RegistrableProvider } from './provider';
import type { AwsRegion, OpenBaoAuthMethod, RegistrationInput } from './registration';

export interface RegistrationDraft {
  readonly displayName: string;
  readonly region: AwsRegion;
  readonly roleArn: string;
  readonly projectId: string;
  readonly location: string;
  readonly workloadIdentityProvider: string;
  readonly impersonatedServiceAccount: string;
  readonly endpoint: string;
  readonly namespace: string;
  readonly transitMount: string;
  readonly authMethod: OpenBaoAuthMethod;
  readonly authRole: string;
  readonly trustRootPem: string;
  readonly version: string;
}

export const emptyRegistrationDraft: RegistrationDraft = {
  displayName: '',
  region: 'eu-west-1',
  roleArn: '',
  projectId: '',
  location: '',
  workloadIdentityProvider: '',
  impersonatedServiceAccount: '',
  endpoint: '',
  namespace: '',
  transitMount: 'transit',
  authMethod: 'jwt',
  authRole: '',
  trustRootPem: '',
  version: '',
};

export function registrationInputOf(provider: RegistrableProvider, draft: RegistrationDraft): RegistrationInput {
  if (provider === KeyProvider.AWS_KMS) {
    return { provider, displayName: draft.displayName, region: draft.region, roleArn: draft.roleArn };
  }
  if (provider === KeyProvider.GOOGLE_CLOUD_KMS) {
    return {
      provider,
      displayName: draft.displayName,
      projectId: draft.projectId,
      location: draft.location,
      workloadIdentityProvider: draft.workloadIdentityProvider,
      impersonatedServiceAccount: draft.impersonatedServiceAccount,
    };
  }
  return {
    provider,
    displayName: draft.displayName,
    endpoint: draft.endpoint,
    namespace: draft.namespace,
    transitMount: draft.transitMount,
    authMethod: draft.authMethod,
    authRole: draft.authRole,
    trustRootPem: draft.trustRootPem,
    version: draft.version,
  };
}
