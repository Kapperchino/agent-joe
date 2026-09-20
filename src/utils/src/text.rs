pub fn preview(content: &str, bytes: usize) -> String {
    match content.len() > bytes {
        false => content.to_owned(),
        true => {
            let half = bytes.saturating_sub(80) / 2;
            let start = content.floor_char_boundary(half);
            let end = content.ceil_char_boundary(content.len() - half);
            format!(
                "{}\n[{} bytes omitted]\n{}",
                &content[..start],
                end - start,
                &content[end..]
            )
        }
    }
}
