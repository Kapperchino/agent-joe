use super::*;
use std::time::Duration;

#[test]
fn process_limits_allow_long_builds_without_removing_time_and_output_bounds() {
    for timeout in [
        Duration::from_secs(1),
        Duration::from_secs(1800),
        ProcessLimits::MAX_TIMEOUT,
    ] {
        assert_eq!(ProcessLimits::new(timeout, 1024).unwrap().timeout, timeout);
    }
    for timeout in [
        Duration::ZERO,
        ProcessLimits::MAX_TIMEOUT + Duration::from_secs(1),
    ] {
        assert!(ProcessLimits::new(timeout, 1024).is_err());
    }
    for bytes in [0, 16 * 1024 * 1024 + 1] {
        assert!(ProcessLimits::new(Duration::from_secs(1), bytes).is_err());
    }
}
