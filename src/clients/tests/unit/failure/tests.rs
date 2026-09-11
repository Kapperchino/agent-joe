use super::*;

#[test]
fn transport_failures_preserve_the_underlying_cause() {
    let error = anyhow::Error::new(std::io::Error::new(
        std::io::ErrorKind::ConnectionReset,
        "connection reset by peer",
    ))
    .context("error decoding response body");

    let failure = Failure::from_error(error);

    assert_eq!(failure.kind, FailureKind::Transport);
    assert_eq!(
        failure.message,
        "error decoding response body: connection reset by peer"
    );
    assert!(failure.retryable());
}

#[test]
fn contextual_failures_preserve_their_classification_and_message() {
    for kind in [
        FailureKind::Authentication,
        FailureKind::RateLimit,
        FailureKind::InvalidInput,
    ] {
        let original = Failure::new(kind, "provider failure");
        let retryable = original.retryable();
        let error = anyhow::Error::new(original).context("provider request failed");

        let failure = Failure::from_error(error);

        assert_eq!(failure.kind, kind);
        assert_eq!(failure.message, "provider failure");
        assert_eq!(failure.retryable(), retryable);
    }
}

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
