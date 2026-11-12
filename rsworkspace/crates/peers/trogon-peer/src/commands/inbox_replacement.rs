use trogonai_proto::peers::v1alpha1 as proto;

pub(super) struct InboxReplacement {
    pub(super) slot: usize,
    pub(super) entry: proto::InboxEntry,
}
