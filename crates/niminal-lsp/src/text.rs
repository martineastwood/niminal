//! Converting between byte offsets and LSP positions (zero-based line, UTF-16 column).

use serde_json::{Value, json};

pub fn position(text: &str, offset: usize) -> Value {
    let offset = offset.min(text.len());
    let line_start = text[..offset].rfind('\n').map_or(0, |i| i + 1);
    let line = text[..line_start].matches('\n').count();
    let character: usize = text[line_start..offset].chars().map(char::len_utf16).sum();
    json!({ "line": line, "character": character })
}

pub fn range(text: &str, start: usize, end: usize) -> Value {
    json!({ "start": position(text, start), "end": position(text, end.max(start)) })
}

/// The byte offset of an LSP position; past the end of a line is the end of the line.
pub fn offset(text: &str, position: &Value) -> usize {
    let line = position["line"].as_u64().unwrap_or(0) as usize;
    let character = position["character"].as_u64().unwrap_or(0) as usize;
    let mut start = 0;
    for _ in 0..line {
        match text[start..].find('\n') {
            Some(i) => start += i + 1,
            None => return text.len(),
        }
    }
    let end = text[start..].find('\n').map_or(text.len(), |i| start + i);
    let mut units = 0;
    for (i, c) in text[start..end].char_indices() {
        if units >= character {
            return start + i;
        }
        units += c.len_utf16();
    }
    end
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The identifier the offset is in or at the end of, with its byte range.
pub fn word_at(text: &str, offset: usize) -> Option<(&str, usize, usize)> {
    let offset = offset.min(text.len());
    let start = text[..offset].char_indices().rev().take_while(|(_, c)| is_word(*c)).last().map_or(offset, |(i, _)| i);
    let end = offset + text[offset..].char_indices().find(|(_, c)| !is_word(*c)).map_or(text.len() - offset, |(i, _)| i);
    (start < end).then(|| (&text[start..end], start, end))
}

/// The directory a `file://` URI is in, for finding sample files.
pub fn directory_of(uri: &str) -> std::path::PathBuf {
    let Some(path) = uri.strip_prefix("file://") else { return ".".into() };
    let mut bytes = Vec::new();
    let raw = path.as_bytes();
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == b'%'
            && let Some(byte) = path.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            bytes.push(byte);
            i += 3;
        } else {
            bytes.push(raw[i]);
            i += 1;
        }
    }
    let path = std::path::PathBuf::from(String::from_utf8_lossy(&bytes).into_owned());
    path.parent().map_or_else(|| ".".into(), std::path::Path::to_path_buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_round_trip_with_utf16_columns() {
        let text = "ab\nc🎵d\nlast";
        for offset in [0, 2, 3, 4, 8, 9, 12] {
            assert_eq!(offset_of(text, offset), offset, "{offset}");
        }
        assert_eq!(position(text, 8), json!({"line": 1, "character": 3}));
        assert_eq!(offset(text, &json!({"line": 9, "character": 0})), text.len());
        assert_eq!(offset(text, &json!({"line": 0, "character": 99})), 2);
    }

    fn offset_of(text: &str, o: usize) -> usize {
        offset(text, &position(text, o))
    }

    #[test]
    fn words_and_directories() {
        let text = "osc(saw, freq)";
        assert_eq!(word_at(text, 5), Some(("saw", 4, 7)));
        assert_eq!(word_at(text, 7), Some(("saw", 4, 7)));
        assert_eq!(word_at(text, 3), Some(("osc", 0, 3)));
        assert_eq!(word_at("a  b", 2), None);
        assert_eq!(directory_of("file:///home/me/my%20set/a.nml"), std::path::PathBuf::from("/home/me/my set"));
        assert_eq!(directory_of("untitled:Untitled-1"), std::path::PathBuf::from("."));
    }
}
