use std::net::IpAddr;

/// Log a ReloadConfig operation for audit purposes.
pub fn audit_reload_config(peer: IpAddr, config_path: &str, success: bool) {
    tracing::info!(
        operation = "ReloadConfig",
        peer = %peer,
        config_path = config_path,
        success = success,
        "audit: config reload triggered"
    );
}

/// Log a GetStatus query for audit purposes.
pub fn audit_get_status(peer: IpAddr) {
    tracing::info!(
        operation = "GetStatus",
        peer = %peer,
        "audit: status query"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::IpAddr;

    #[test]
    fn test_audit_reload_config_does_not_panic() {
        audit_reload_config(IpAddr::from([127, 0, 0, 1]), "/etc/config.toml", true);
    }

    #[test]
    fn test_audit_get_status_does_not_panic() {
        audit_get_status(IpAddr::from([10, 0, 0, 1]));
    }
}
