use trogon_std::{ByteSize, HttpBodySizeMax};

pub const HTTP_BODY_SIZE_MAX: HttpBodySizeMax = HttpBodySizeMax::new(ByteSize::mib(25)).unwrap();

pub const HEADER_SIGNATURE: &str = "x-hub-signature-256";
pub const HEADER_EVENT: &str = "x-github-event";
pub const HEADER_DELIVERY: &str = "x-github-delivery";

pub const NATS_HEADER_EVENT: &str = "X-GitHub-Event";
pub const NATS_HEADER_DELIVERY: &str = "X-GitHub-Delivery";
