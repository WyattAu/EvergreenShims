use tonic::Status;

use crate::proto::{
    GetMetricsRequest, GetStatusRequest, ListCapabilitiesRequest, ReloadConfigRequest,
};

const MAX_STRING_LENGTH: usize = 1024;
const MAX_PORT: u32 = 65535;

fn is_valid_metric_name(name: &str) -> bool {
    if name.is_empty() || name.len() > MAX_STRING_LENGTH {
        return false;
    }
    name.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
}

#[allow(clippy::result_large_err)]
fn validate_string_field(value: &str, field_name: &str, required: bool) -> Result<(), Status> {
    if value.is_empty() {
        if required {
            return Err(Status::invalid_argument(format!(
                "{field_name} must not be empty"
            )));
        }
        return Ok(());
    }
    if value.len() > MAX_STRING_LENGTH {
        return Err(Status::invalid_argument(format!(
            "{field_name} exceeds maximum length of {MAX_STRING_LENGTH}"
        )));
    }
    if value.contains('\0') {
        return Err(Status::invalid_argument(format!(
            "{field_name} contains null byte"
        )));
    }
    Ok(())
}

pub trait Validate {
    #[allow(clippy::result_large_err)]
    fn validate(&self) -> Result<(), Status>;
}

impl Validate for GetStatusRequest {
    fn validate(&self) -> Result<(), Status> {
        Ok(())
    }
}

impl Validate for GetMetricsRequest {
    fn validate(&self) -> Result<(), Status> {
        Ok(())
    }
}

impl Validate for ReloadConfigRequest {
    fn validate(&self) -> Result<(), Status> {
        validate_string_field(&self.config_path, "config_path", false)?;
        Ok(())
    }
}

impl Validate for ListCapabilitiesRequest {
    fn validate(&self) -> Result<(), Status> {
        Ok(())
    }
}

#[allow(clippy::result_large_err)]
pub fn validate_port(port: u32) -> Result<(), Status> {
    if port == 0 || port > MAX_PORT {
        return Err(Status::invalid_argument(format!(
            "port must be between 1 and {MAX_PORT}, got {port}"
        )));
    }
    Ok(())
}

#[allow(clippy::result_large_err)]
pub fn validate_metric_name(name: &str) -> Result<(), Status> {
    if !is_valid_metric_name(name) {
        return Err(Status::invalid_argument(format!(
            "invalid metric name: '{name}'"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_status_request_valid() {
        assert!(GetStatusRequest {}.validate().is_ok());
    }

    #[test]
    fn test_get_metrics_request_valid() {
        assert!(GetMetricsRequest {}.validate().is_ok());
    }

    #[test]
    fn test_list_capabilities_request_valid() {
        assert!(ListCapabilitiesRequest {}.validate().is_ok());
    }

    #[test]
    fn test_reload_config_empty_path_ok() {
        let req = ReloadConfigRequest {
            config_path: String::new(),
        };
        assert!(req.validate().is_ok());
    }

    #[test]
    fn test_reload_config_valid_path() {
        let req = ReloadConfigRequest {
            config_path: "/etc/config.toml".to_string(),
        };
        assert!(req.validate().is_ok());
    }

    #[test]
    fn test_reload_config_too_long() {
        let req = ReloadConfigRequest {
            config_path: "a".repeat(MAX_STRING_LENGTH + 1),
        };
        assert!(req.validate().is_err());
    }

    #[test]
    fn test_reload_config_null_byte() {
        let req = ReloadConfigRequest {
            config_path: "path\0bad".to_string(),
        };
        assert!(req.validate().is_err());
    }

    #[test]
    fn test_validate_port_valid() {
        assert!(validate_port(80).is_ok());
        assert!(validate_port(65535).is_ok());
        assert!(validate_port(1).is_ok());
    }

    #[test]
    fn test_validate_port_invalid() {
        assert!(validate_port(0).is_err());
        assert!(validate_port(65536).is_err());
    }

    #[test]
    fn test_validate_metric_name_valid() {
        assert!(validate_metric_name("shim_uptime_seconds").is_ok());
        assert!(validate_metric_name("cpu.usage").is_ok());
        assert!(validate_metric_name("a").is_ok());
    }

    #[test]
    fn test_validate_metric_name_invalid() {
        assert!(validate_metric_name("").is_err());
        assert!(validate_metric_name("has space").is_err());
        assert!(validate_metric_name("has-dash").is_err());
        assert!(validate_metric_name(&"a".repeat(MAX_STRING_LENGTH + 1)).is_err());
    }
}
