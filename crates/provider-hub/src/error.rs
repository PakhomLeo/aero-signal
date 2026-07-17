use thiserror::Error;

/// Stable handling decision exposed to provider consumers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderErrorClass {
    Retryable,
    Permanent,
    NeedsAction,
}

#[derive(Error, Debug, Clone)]
pub enum ProviderError {
    #[error("{code}: {message}")]
    Retryable {
        code: String,
        message: String,
        delay_secs: u64,
    },
    #[error("{code}: {message}")]
    Permanent { code: String, message: String },
    #[error("{code}: {message}")]
    NeedsAction { code: String, message: String },
}

impl ProviderError {
    pub fn code(&self) -> &str {
        match self {
            ProviderError::Retryable { code, .. }
            | ProviderError::Permanent { code, .. }
            | ProviderError::NeedsAction { code, .. } => code,
        }
    }

    pub fn permanent(code: impl Into<String>, message: impl Into<String>) -> Self {
        ProviderError::Permanent {
            code: code.into(),
            message: message.into(),
        }
    }

    pub fn retryable(code: impl Into<String>, message: impl Into<String>, delay_secs: u64) -> Self {
        ProviderError::Retryable {
            code: code.into(),
            message: message.into(),
            delay_secs,
        }
    }

    pub fn needs_action(code: impl Into<String>, message: impl Into<String>) -> Self {
        ProviderError::NeedsAction {
            code: code.into(),
            message: message.into(),
        }
    }

    pub fn class(&self) -> ProviderErrorClass {
        match self {
            ProviderError::Retryable { .. } => ProviderErrorClass::Retryable,
            ProviderError::Permanent { .. } => ProviderErrorClass::Permanent,
            ProviderError::NeedsAction { .. } => ProviderErrorClass::NeedsAction,
        }
    }

    pub fn is_retryable(&self) -> bool {
        self.class() == ProviderErrorClass::Retryable
    }

    pub fn retry_after_secs(&self) -> Option<u64> {
        match self {
            ProviderError::Retryable { delay_secs, .. } => Some(*delay_secs),
            _ => None,
        }
    }

    pub fn auth_failed() -> Self {
        ProviderError::Permanent {
            code: "provider_hub.auth_failed".into(),
            message: "Authentication failed".into(),
        }
    }
}

pub type ProviderResult<T> = Result<T, ProviderError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_have_stable_handling_classes() {
        let retryable = ProviderError::retryable("provider.timeout", "timed out", 7);
        assert_eq!(retryable.class(), ProviderErrorClass::Retryable);
        assert!(retryable.is_retryable());
        assert_eq!(retryable.retry_after_secs(), Some(7));

        let permanent = ProviderError::permanent("provider.invalid", "invalid request");
        assert_eq!(permanent.class(), ProviderErrorClass::Permanent);
        assert!(!permanent.is_retryable());
        assert_eq!(permanent.retry_after_secs(), None);

        let action = ProviderError::needs_action("provider.auth", "refresh credentials");
        assert_eq!(action.class(), ProviderErrorClass::NeedsAction);
        assert!(!action.is_retryable());
    }
}
