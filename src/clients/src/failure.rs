use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    Authentication,
    RateLimit,
    Transport,
    Truncation,
    ContextOverflow,
    InvalidInput,
    Tool,
    Worker,
}

#[derive(Debug, Clone)]
pub struct Failure {
    pub kind: FailureKind,
    pub message: String,
}
impl Failure {
    pub fn new(kind: FailureKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
    pub fn http(status: u16, body: String) -> Self {
        let kind = match status {
            401 | 403 => FailureKind::Authentication,
            429 => FailureKind::RateLimit,
            400 | 404 | 413 | 422 => {
                let error = serde_json::from_str::<HttpError>(&body).ok();
                error
                    .and_then(|error| {
                        error
                            .error
                            .code
                            .kind()
                            .or_else(|| error.error.error_type.kind())
                    })
                    .unwrap_or(FailureKind::InvalidInput)
            }
            _ => FailureKind::Transport,
        };
        Self::new(kind, body)
    }
    pub fn api(code: &str, message: &str) -> Self {
        let code_value = serde_json::Value::String(code.to_owned());
        let kind = serde_json::from_value::<ProviderErrorCode>(code_value)
            .unwrap_or_default()
            .kind()
            .unwrap_or(FailureKind::InvalidInput);
        Self::new(kind, format!("{code}: {message}"))
    }
    pub fn from_error(error: anyhow::Error) -> Self {
        error
            .downcast_ref::<Self>()
            .cloned()
            .unwrap_or_else(|| Self::new(FailureKind::Transport, error.to_string()))
    }
    pub fn retryable(&self) -> bool {
        matches!(self.kind, FailureKind::Transport | FailureKind::RateLimit)
    }
}
impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.message)
    }
}
impl std::error::Error for Failure {}

#[derive(serde::Deserialize)]
struct HttpError {
    error: HttpErrorDetail,
}
#[derive(serde::Deserialize)]
struct HttpErrorDetail {
    #[serde(default, deserialize_with = "nullable_code")]
    code: ProviderErrorCode,
    #[serde(default, rename = "type", deserialize_with = "nullable_code")]
    error_type: ProviderErrorCode,
}
fn nullable_code<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<ProviderErrorCode, D::Error> {
    use serde::Deserialize;
    Option::<ProviderErrorCode>::deserialize(deserializer).map(Option::unwrap_or_default)
}
#[derive(Default, serde::Deserialize)]
enum ProviderErrorCode {
    #[serde(
        rename = "authentication_error",
        alias = "invalid_api_key",
        alias = "permission_error"
    )]
    Authentication,
    #[serde(
        rename = "rate_limit_error",
        alias = "rate_limit_exceeded",
        alias = "insufficient_quota"
    )]
    RateLimit,
    #[serde(
        rename = "overloaded_error",
        alias = "server_error",
        alias = "api_error",
        alias = "failed_response"
    )]
    Transport,
    #[serde(
        rename = "context_length_exceeded",
        alias = "context_window_exceeded",
        alias = "context_exceeded"
    )]
    ContextOverflow,
    #[serde(
        rename = "incomplete_response",
        alias = "max_tokens",
        alias = "max_output_tokens"
    )]
    Truncation,
    #[default]
    #[serde(other)]
    Unknown,
}
impl ProviderErrorCode {
    fn kind(&self) -> Option<FailureKind> {
        match self {
            Self::Authentication => Some(FailureKind::Authentication),
            Self::RateLimit => Some(FailureKind::RateLimit),
            Self::Transport => Some(FailureKind::Transport),
            Self::ContextOverflow => Some(FailureKind::ContextOverflow),
            Self::Truncation => Some(FailureKind::Truncation),
            Self::Unknown => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn diagnostic_text_cannot_change_recovery_policy() {
        let misleading = "authentication error, context overflow, timeout, rate limit";
        assert_eq!(
            Failure::api("server_error", misleading).kind,
            FailureKind::Transport
        );
        assert_eq!(
            Failure::api("unknown_error", misleading).kind,
            FailureKind::InvalidInput
        );
        assert_eq!(
            Failure::http(400, misleading.into()).kind,
            FailureKind::InvalidInput
        );
        assert_eq!(
            Failure::http(
                400,
                r#"{"error":{"code":null,"type":"context_length_exceeded"}}"#.into()
            )
            .kind,
            FailureKind::ContextOverflow
        );
        assert_eq!(
            Failure::http(
                400,
                r#"{"error":{"code":"unknown","message":"rate_limit_error"}}"#.into()
            )
            .kind,
            FailureKind::InvalidInput
        );
    }

    #[test]
    fn classifies_provider_failures_and_limits_automatic_recovery() {
        for (status, message, expected) in [
            (401, "unauthorized", FailureKind::Authentication),
            (429, "slow down", FailureKind::RateLimit),
            (503, "unavailable", FailureKind::Transport),
            (
                400,
                r#"{"error":{"code":"context_length_exceeded"}}"#,
                FailureKind::ContextOverflow,
            ),
            (422, "bad request", FailureKind::InvalidInput),
        ] {
            assert_eq!(Failure::http(status, message.into()).kind, expected);
        }
        assert_eq!(
            Failure::api("incomplete_response", "max_output_tokens").kind,
            FailureKind::Truncation
        );
        assert!(Failure::new(FailureKind::Transport, "reset").retryable());
        for kind in [
            FailureKind::Authentication,
            FailureKind::Truncation,
            FailureKind::ContextOverflow,
            FailureKind::InvalidInput,
            FailureKind::Tool,
            FailureKind::Worker,
        ] {
            assert!(!Failure::new(kind, "failure").retryable());
        }
    }
}
