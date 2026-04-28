use trogon_std::{ByteSize, HttpBodySizeMax};

pub const HTTP_BODY_SIZE_MAX: HttpBodySizeMax = HttpBodySizeMax::new(ByteSize::mib(2)).unwrap();

pub const NATS_HEADER_CHANGE_TYPE: &str = "X-Microsoft-Teams-Change-Type";
pub const NATS_HEADER_NOTIFICATION_ID: &str = "X-Microsoft-Teams-Notification-Id";
pub const NATS_HEADER_REJECT_REASON: &str = "X-Microsoft-Teams-Reject-Reason";
pub const NATS_HEADER_RESOURCE: &str = "X-Microsoft-Teams-Resource";
pub const NATS_HEADER_RESOURCE_ID: &str = "X-Microsoft-Teams-Resource-Id";
pub const NATS_HEADER_RESOURCE_TYPE: &str = "X-Microsoft-Teams-Resource-Type";
pub const NATS_HEADER_SUBSCRIPTION_EXPIRATION: &str = "X-Microsoft-Teams-Subscription-Expiration";
pub const NATS_HEADER_SUBSCRIPTION_ID: &str = "X-Microsoft-Teams-Subscription-Id";
pub const NATS_HEADER_TENANT_ID: &str = "X-Microsoft-Teams-Tenant-Id";
