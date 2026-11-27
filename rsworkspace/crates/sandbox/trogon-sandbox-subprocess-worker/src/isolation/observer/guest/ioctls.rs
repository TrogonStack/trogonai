use nix::{ioctl_readwrite, ioctl_write_ptr};

use super::{Notification, Response};

ioctl_readwrite!(receive, b'!', 0, Notification);
ioctl_readwrite!(send, b'!', 1, Response);
ioctl_write_ptr!(id_valid, b'!', 2, u64);
