pub(super) const PEER_NOTICE: &str = "Another Claude session sent a message";

pub(super) const ARRIVED_PREFIX: &str = "A message arrived from ";

pub(super) const WORKER_PREFIXES: [&str; 7] = [
    "<bash-stdout",
    "<bash-stderr",
    "<local-command-stdout",
    "<local-command-stderr",
    "<channel source=\"",
    "<teammate-message ",
    "<teammate-message>",
];

pub(super) const NOTICE_PREFIXES: [&str; 3] = [
    "Another Claude session sent a message while you were working:\n",
    "Another Claude session sent a message:\n",
    "A peer session sent a message while you were working:\n",
];

pub(super) const EMBEDDED_MARKERS: [&str; 8] = [
    "<bash-input>",
    "<command-message>",
    "<user-memory-input>",
    "<task-notification",
    "<mcp-resource-update",
    "<mcp-polling-update",
    "<fork-boilerplate>",
    "<local-command-caveat>",
];
