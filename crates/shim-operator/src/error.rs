//! Error types for the shim operator.

/// Operator error type.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum OperatorError {
    /// Kubernetes API error.
    #[error("kubernetes error: {0}")]
    Kubernetes(#[from] kube::Error),

    /// CRD validation error.
    #[error("validation error: {0}")]
    Validation(String),

    /// Reconciliation error (retriable).
    #[error("reconciliation error: {0}")]
    Reconcile(String),

    /// Serialization error.
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    /// ConfigMap generation error.
    #[error("configmap generation error: {0}")]
    ConfigMapGen(String),

    /// Deployment update error.
    #[error("deployment update error: {0}")]
    DeploymentUpdate(String),

    /// I/O error.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

impl OperatorError {
    /// Create a validation error.
    #[allow(dead_code)]
    pub fn validation(msg: impl Into<String>) -> Self {
        Self::Validation(msg.into())
    }

    /// Create a reconcile error.
    #[allow(dead_code)]
    pub fn reconcile(msg: impl Into<String>) -> Self {
        Self::Reconcile(msg.into())
    }

    /// Create a configmap generation error.
    #[allow(dead_code)]
    pub fn configmap_gen(msg: impl Into<String>) -> Self {
        Self::ConfigMapGen(msg.into())
    }

    /// Create a deployment update error.
    #[allow(dead_code)]
    pub fn deployment_update(msg: impl Into<String>) -> Self {
        Self::DeploymentUpdate(msg.into())
    }
}

/// Result type for operator operations.
pub type OperatorResult<T> = std::result::Result<T, OperatorError>;

/// Error classification for retry decisions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErrorClass {
    /// Transient error — safe to retry with backoff.
    Transient,
    /// Permanent error — should not retry.
    Permanent,
}

impl OperatorError {
    /// Classify the error for retry decisions.
    pub fn classify(&self) -> ErrorClass {
        match self {
            OperatorError::Kubernetes(e) => {
                if is_transient_kube_error(e) {
                    ErrorClass::Transient
                } else {
                    ErrorClass::Permanent
                }
            }
            OperatorError::Validation(_) => ErrorClass::Permanent,
            OperatorError::Reconcile(_) => ErrorClass::Transient,
            OperatorError::Serialization(_) => ErrorClass::Permanent,
            OperatorError::ConfigMapGen(_) => ErrorClass::Permanent,
            OperatorError::DeploymentUpdate(_) => ErrorClass::Transient,
            OperatorError::Io(_) => ErrorClass::Transient,
        }
    }
}

fn is_transient_kube_error(e: &kube::Error) -> bool {
    // kube::Error doesn't have named variants in all versions;
    // use the Display representation for classification
    let msg = e.to_string();
    let lower = msg.to_lowercase();
    // Transient patterns
    lower.contains("connection")
        || lower.contains("timeout")
        || lower.contains("reset")
        || lower.contains("broken pipe")
        || lower.contains("request send")
        || lower.contains("response")
        || lower.contains("recv")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validation_error_is_permanent() {
        let err = OperatorError::validation("bad input");
        assert_eq!(err.classify(), ErrorClass::Permanent);
    }

    #[test]
    fn test_reconcile_error_is_transient() {
        let err = OperatorError::reconcile("retry me");
        assert_eq!(err.classify(), ErrorClass::Transient);
    }

    #[test]
    fn test_configmap_gen_error_is_permanent() {
        let err = OperatorError::configmap_gen("bad spec");
        assert_eq!(err.classify(), ErrorClass::Permanent);
    }
}
