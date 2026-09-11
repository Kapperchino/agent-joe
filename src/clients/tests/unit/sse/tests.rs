use super::*;
#[test]
fn unicode_framing_and_keepalives_at_every_chunk_boundary() {
    let input = ": ping\r\n\r\nevent: msg\r\ndata:{\"text\":\"終😀\",\r\ndata: \"ok\":true}\r\n\r\ndata: [DONE]\n\n";
    for split in 0..=input.len() {
        let mut decoder = Decoder::default();
        let mut events = decoder.push(&input.as_bytes()[..split]).unwrap();
        events.extend(decoder.push(&input.as_bytes()[split..]).unwrap());
        decoder.finish().unwrap();
        assert_eq!(events, ["{\"text\":\"終😀\",\n\"ok\":true}", "[DONE]"]);
    }
}
#[test]
fn rejects_partial_event_and_invalid_utf8() {
    let mut decoder = Decoder::default();
    decoder.push(b"data: {}\n").unwrap();
    assert!(decoder.finish().is_err());
    assert!(Decoder::default().push(b"data: \xff\n\n").is_err());
}
