import {
  CredentialLifecycle,
  CredentialPolicyReason,
  CredentialWriteKind,
  MaterialVerificationOutcome,
  SecretAuthorizationEndReason,
  SecretLeaseEndReason,
  SecretMaterialKind,
  SecretMaterialOrigin,
  SecretPurpose,
  SecretUseAction,
  SecretUseDirection,
  SecretUseMode,
  SecretUseResult,
  SecretVersionState,
  VaultLifecycle,
} from '@trogonai/proto';

export type LedTone = 'success' | 'warning' | 'info' | 'danger' | 'neutral';

export const purposeLabels: Record<SecretPurpose, string> = {
  [SecretPurpose.UNSPECIFIED]: 'Unspecified',
  [SecretPurpose.PROVIDER_AUTHENTICATION]: 'Provider authentication',
  [SecretPurpose.WEBHOOK_VERIFICATION]: 'Webhook verification',
  [SecretPurpose.WEBHOOK_SIGNING]: 'Webhook signing',
  [SecretPurpose.OAUTH_REFRESH]: 'OAuth refresh',
  [SecretPurpose.WEBHOOK_DESTINATION]: 'Webhook destination',
  [SecretPurpose.TOOL_AUTHENTICATION]: 'Tool authentication',
  [SecretPurpose.MCP_AUTHENTICATION]: 'MCP authentication',
};

export const purposeDescriptions: Record<SecretPurpose, string> = {
  [SecretPurpose.UNSPECIFIED]: '',
  [SecretPurpose.PROVIDER_AUTHENTICATION]: 'Authenticates requests to a model provider.',
  [SecretPurpose.WEBHOOK_VERIFICATION]: 'Verifies the signature of inbound webhook deliveries.',
  [SecretPurpose.WEBHOOK_SIGNING]: 'Signs outbound webhook deliveries.',
  [SecretPurpose.OAUTH_REFRESH]: 'Refreshes an OAuth grant held on the provider side.',
  [SecretPurpose.WEBHOOK_DESTINATION]: 'A destination URL that must not be disclosed.',
  [SecretPurpose.TOOL_AUTHENTICATION]: 'Authenticates calls a tool makes to its upstream.',
  [SecretPurpose.MCP_AUTHENTICATION]: 'Authenticates a connection to an MCP server.',
};

export const kindLabels: Record<SecretMaterialKind, string> = {
  [SecretMaterialKind.UNSPECIFIED]: 'Unspecified',
  [SecretMaterialKind.API_TOKEN]: 'API token',
  [SecretMaterialKind.BASIC_AUTHENTICATION]: 'Basic authentication',
  [SecretMaterialKind.OAUTH_ACCESS_TOKEN]: 'OAuth access token',
  [SecretMaterialKind.OAUTH_REFRESH_TOKEN]: 'OAuth refresh token',
  [SecretMaterialKind.WEBHOOK_SHARED_SECRET]: 'Webhook shared secret',
  [SecretMaterialKind.WEBHOOK_DESTINATION]: 'Webhook destination',
};

export const originLabels: Record<SecretMaterialOrigin, string> = {
  [SecretMaterialOrigin.UNSPECIFIED]: 'Unspecified',
  [SecretMaterialOrigin.IMPORTED]: 'Imported',
  [SecretMaterialOrigin.GENERATED]: 'Generated',
  [SecretMaterialOrigin.PROVIDER_ISSUED]: 'Provider issued',
};

export const credentialLifecycleLabels: Record<CredentialLifecycle, string> = {
  [CredentialLifecycle.UNSPECIFIED]: 'Unspecified',
  [CredentialLifecycle.PROVISIONING]: 'Provisioning',
  [CredentialLifecycle.ACTIVE]: 'Active',
  [CredentialLifecycle.DISABLED]: 'Disabled',
  [CredentialLifecycle.REVOKED]: 'Revoked',
  [CredentialLifecycle.DESTROY_PENDING]: 'Destroy pending',
  [CredentialLifecycle.DESTROYED]: 'Destroyed',
  [CredentialLifecycle.RECONNECT_REQUIRED]: 'Reconnect required',
  [CredentialLifecycle.CLEANUP_PENDING]: 'Cleanup pending',
  [CredentialLifecycle.PROVISIONING_ABORTED]: 'Provisioning aborted',
};

export const credentialLifecycleTones: Record<CredentialLifecycle, LedTone> = {
  [CredentialLifecycle.UNSPECIFIED]: 'neutral',
  [CredentialLifecycle.PROVISIONING]: 'warning',
  [CredentialLifecycle.ACTIVE]: 'success',
  [CredentialLifecycle.DISABLED]: 'neutral',
  [CredentialLifecycle.REVOKED]: 'danger',
  [CredentialLifecycle.DESTROY_PENDING]: 'danger',
  [CredentialLifecycle.DESTROYED]: 'neutral',
  [CredentialLifecycle.RECONNECT_REQUIRED]: 'warning',
  [CredentialLifecycle.CLEANUP_PENDING]: 'info',
  [CredentialLifecycle.PROVISIONING_ABORTED]: 'danger',
};

export const vaultLifecycleLabels: Record<VaultLifecycle, string> = {
  [VaultLifecycle.UNSPECIFIED]: 'Unspecified',
  [VaultLifecycle.ACTIVE]: 'Active',
  [VaultLifecycle.DISABLED]: 'Disabled',
  [VaultLifecycle.DESTROY_PENDING]: 'Destroy pending',
  [VaultLifecycle.DESTROYED]: 'Destroyed',
};

export const vaultLifecycleTones: Record<VaultLifecycle, LedTone> = {
  [VaultLifecycle.UNSPECIFIED]: 'neutral',
  [VaultLifecycle.ACTIVE]: 'success',
  [VaultLifecycle.DISABLED]: 'neutral',
  [VaultLifecycle.DESTROY_PENDING]: 'danger',
  [VaultLifecycle.DESTROYED]: 'neutral',
};

export const versionStateLabels: Record<SecretVersionState, string> = {
  [SecretVersionState.UNSPECIFIED]: 'Unspecified',
  [SecretVersionState.AVAILABLE]: 'Available',
  [SecretVersionState.SOFT_DELETED]: 'Soft deleted',
  [SecretVersionState.DESTROYED]: 'Destroyed',
};

export const versionStateTones: Record<SecretVersionState, LedTone> = {
  [SecretVersionState.UNSPECIFIED]: 'neutral',
  [SecretVersionState.AVAILABLE]: 'success',
  [SecretVersionState.SOFT_DELETED]: 'warning',
  [SecretVersionState.DESTROYED]: 'neutral',
};

export const policyReasonLabels: Record<CredentialPolicyReason, string> = {
  [CredentialPolicyReason.UNSPECIFIED]: 'Unspecified',
  [CredentialPolicyReason.OPERATOR_REQUESTED]: 'Operator requested',
  [CredentialPolicyReason.COMPROMISE]: 'Suspected compromise',
  [CredentialPolicyReason.PROVIDER_AUTHENTICATION_REJECTED]: 'Provider rejected the authentication',
  [CredentialPolicyReason.OWNER_REMOVED]: 'Owner removed',
  [CredentialPolicyReason.OVERLAP_EXPIRED]: 'Overlap window expired',
};

export const selectableRevocationReasons: readonly CredentialPolicyReason[] = [
  CredentialPolicyReason.OPERATOR_REQUESTED,
  CredentialPolicyReason.COMPROMISE,
  CredentialPolicyReason.PROVIDER_AUTHENTICATION_REJECTED,
  CredentialPolicyReason.OWNER_REMOVED,
];

export const writeKindLabels: Record<CredentialWriteKind, string> = {
  [CredentialWriteKind.UNSPECIFIED]: 'Unspecified',
  [CredentialWriteKind.INITIAL]: 'Initial write',
  [CredentialWriteKind.ROTATION]: 'Rotation',
  [CredentialWriteKind.RECONNECT]: 'Reconnect',
};

export const verificationOutcomeLabels: Record<MaterialVerificationOutcome, string> = {
  [MaterialVerificationOutcome.UNSPECIFIED]: 'Unspecified',
  [MaterialVerificationOutcome.PASSED]: 'Passed',
  [MaterialVerificationOutcome.ACCOUNT_MISMATCH]: 'Account mismatch',
  [MaterialVerificationOutcome.AUTHENTICATION_REJECTED]: 'Authentication rejected',
  [MaterialVerificationOutcome.PROVIDER_UNAVAILABLE]: 'Provider unavailable',
  [MaterialVerificationOutcome.ENDPOINT_MISMATCH]: 'Endpoint mismatch',
};

export const selectableVerificationOutcomes: readonly MaterialVerificationOutcome[] = [
  MaterialVerificationOutcome.PASSED,
  MaterialVerificationOutcome.AUTHENTICATION_REJECTED,
  MaterialVerificationOutcome.ACCOUNT_MISMATCH,
  MaterialVerificationOutcome.ENDPOINT_MISMATCH,
  MaterialVerificationOutcome.PROVIDER_UNAVAILABLE,
];

export const useActionLabels: Record<SecretUseAction, string> = {
  [SecretUseAction.UNSPECIFIED]: 'Unspecified',
  [SecretUseAction.RESOLVE]: 'Resolve',
  [SecretUseAction.COMPUTE_MAC]: 'Compute MAC',
  [SecretUseAction.VERIFY_MAC]: 'Verify MAC',
  [SecretUseAction.VERIFY_TOKEN]: 'Verify token',
};

export const useDirectionLabels: Record<SecretUseDirection, string> = {
  [SecretUseDirection.UNSPECIFIED]: 'Unspecified',
  [SecretUseDirection.INBOUND]: 'Inbound',
  [SecretUseDirection.OUTBOUND]: 'Outbound',
};

export const useModeLabels: Record<SecretUseMode, string> = {
  [SecretUseMode.UNSPECIFIED]: 'Unspecified',
  [SecretUseMode.RUNTIME]: 'Runtime',
  [SecretUseMode.MANAGEMENT_VERIFICATION]: 'Management verification',
  [SecretUseMode.MANAGEMENT_PROVISIONING]: 'Management provisioning',
  [SecretUseMode.OWNED_CLEANUP]: 'Owned cleanup',
};

export const useResultLabels: Record<SecretUseResult, string> = {
  [SecretUseResult.UNSPECIFIED]: 'Unspecified',
  [SecretUseResult.MATERIAL_RELEASED]: 'Material released',
  [SecretUseResult.MAC_COMPUTED]: 'MAC computed',
  [SecretUseResult.VERIFICATION_MATCHED]: 'Verification matched',
  [SecretUseResult.VERIFICATION_REJECTED]: 'Verification rejected',
  [SecretUseResult.FAILED]: 'Failed',
};

export const authorizationEndLabels: Record<SecretAuthorizationEndReason, string> = {
  [SecretAuthorizationEndReason.UNSPECIFIED]: 'Unspecified',
  [SecretAuthorizationEndReason.EXPIRED]: 'Expired',
  [SecretAuthorizationEndReason.OPERATION_FINISHED]: 'Operation finished',
  [SecretAuthorizationEndReason.POLICY_REVOKED]: 'Policy revoked',
};

export const leaseEndLabels: Record<SecretLeaseEndReason, string> = {
  [SecretLeaseEndReason.UNSPECIFIED]: 'Unspecified',
  [SecretLeaseEndReason.EXPIRED]: 'Expired',
  [SecretLeaseEndReason.OPERATION_FINISHED]: 'Operation finished',
  [SecretLeaseEndReason.POLICY_REVOKED]: 'Policy revoked',
  [SecretLeaseEndReason.REPLACED]: 'Replaced',
};
