use super::*;

#[test]
fn request_context_reserves_space_for_output() {
    let diff = "-retries = 3\n+retries = 5\n";
    let request = CommitRequest::new(diff.into(), usize::MAX).unwrap();
    let required =
        crate::context::estimated_tokens(&request.request).unwrap() + OUTPUT_TOKENS as usize;
    assert!(CommitRequest::new(diff.into(), required).is_err());
    assert!(CommitRequest::new(diff.into(), required + 1).is_ok());
}

#[test]
fn request_accepts_the_exact_diff_byte_limit_without_truncation() {
    let diff = "x".repeat(MAX_DIFF_BYTES);
    let request = CommitRequest::new(diff.clone(), usize::MAX).unwrap();
    assert_eq!(request.request.messages[0].text(), diff);
}
