const MAX_INPUT_LENGTH: usize = 1024;

/// Strip control characters (except newline and tab) and truncate to MAX_INPUT_LENGTH.
/// Returns None if the input contains a null byte.
pub fn sanitize_input(input: &str) -> Result<String, &'static str> {
    if input.contains('\0') {
        return Err("input contains null byte");
    }

    let sanitized: String = input
        .chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .take(MAX_INPUT_LENGTH)
        .collect();

    Ok(sanitized)
}

/// Sanitize a string field, returning a tonic::Status error on failure.
#[allow(clippy::result_large_err)]
pub fn sanitize_string_field(value: &str, field_name: &str) -> Result<String, tonic::Status> {
    match sanitize_input(value) {
        Ok(sanitized) => Ok(sanitized),
        Err(msg) => Err(tonic::Status::invalid_argument(format!(
            "{field_name}: {msg}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_clean_input() {
        let result = sanitize_input("hello world").unwrap();
        assert_eq!(result, "hello world");
    }

    #[test]
    fn test_sanitize_strips_control_chars() {
        let input = "hello\x01\x02\x03world";
        let result = sanitize_input(input).unwrap();
        assert_eq!(result, "helloworld");
    }

    #[test]
    fn test_sanitize_preserves_newline_and_tab() {
        let input = "hello\tworld\n";
        let result = sanitize_input(input).unwrap();
        assert_eq!(result, "hello\tworld\n");
    }

    #[test]
    fn test_sanitize_rejects_null_byte() {
        let result = sanitize_input("hello\0world");
        assert!(result.is_err());
    }

    #[test]
    fn test_sanitize_truncates_long_input() {
        let input = "a".repeat(2000);
        let result = sanitize_input(&input).unwrap();
        assert_eq!(result.len(), MAX_INPUT_LENGTH);
    }

    #[test]
    fn test_sanitize_empty_input() {
        let result = sanitize_input("").unwrap();
        assert_eq!(result, "");
    }

    #[test]
    fn test_sanitize_string_field_ok() {
        let result = sanitize_string_field("config.toml", "config_path").unwrap();
        assert_eq!(result, "config.toml");
    }

    #[test]
    fn test_sanitize_string_field_null_byte() {
        let result = sanitize_string_field("bad\0input", "config_path");
        assert!(result.is_err());
    }

    #[test]
    fn test_sanitize_string_field_control_chars() {
        let result = sanitize_string_field("hello\x01\x02", "field").unwrap();
        assert_eq!(result, "hello");
    }
}
