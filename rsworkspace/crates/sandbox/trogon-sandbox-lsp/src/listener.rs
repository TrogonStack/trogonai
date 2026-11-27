use crate::notification_handle::NotificationHandle;
use crate::notification_handler::NotificationHandler;

pub(crate) struct Listener {
    pub(crate) id: u64,
    pub(crate) method: String,
    pub(crate) handler: NotificationHandler,
    pub(crate) attached: Option<NotificationHandle>,
}
