//! MCP servers emit `notifications/tools/list_changed` when their tool catalog changes;
//! the gateway must drop cached schemas for that server. Subscribe wiring is follow-up work.

pub fn should_invalidate(notification_method: &str) -> bool {
    notification_method == "notifications/tools/list_changed"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_invalidate_only_for_tools_list_changed() {
        assert!(should_invalidate("notifications/tools/list_changed"));
        assert!(!should_invalidate("notifications/tools/list_changed/"));
        assert!(!should_invalidate("tools/list_changed"));
        assert!(!should_invalidate(""));
    }
}
