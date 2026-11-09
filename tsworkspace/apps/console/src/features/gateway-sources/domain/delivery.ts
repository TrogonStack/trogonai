export type RejectionReason = 'invalid_signature' | 'stale_timestamp' | 'key_unavailable';

export type DeliveryOutcome =
  | { readonly status: 'accepted' }
  | { readonly status: 'rejected'; readonly reason: RejectionReason };

export interface DeliveryRecord {
  readonly id: string;
  readonly receivedAt: Date;
  readonly event: string;
  readonly outcome: DeliveryOutcome;
}

export interface HourlyDeliveries {
  readonly accepted: number;
  readonly rejected: number;
}

export const HOURS_IN_WINDOW = 24;

export const rejectionLabels: Record<RejectionReason, string> = {
  invalid_signature: 'Invalid signature',
  stale_timestamp: 'Stale timestamp',
  key_unavailable: 'Key unavailable',
};
