//! File-type sniffing for Assets whose contents carry no declared format:
//! TextAssets and the raw data of AudioClips.

/// The file extension (without a dot) that suits a TextAsset's bytes:
/// `json`, `xml`, `yaml`, `txt`, or `bytes` for anything that is not text.
///
/// JSON is recognised only when the whole text parses; XML by a leading
/// `<?xml` or a leading tag with a closing `>` at the end; YAML by a leading
/// `%YAML` directive or `---` document marker. Other valid UTF-8 without
/// control characters (tabs, newlines and form feeds allowed) is `txt`, as is
/// text with a UTF-16 byte-order mark.
#[must_use]
pub fn sniff_text_extension(bytes: &[u8]) -> &'static str {
    if bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF]) {
        return "txt";
    }
    let body = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    let Ok(text) = std::str::from_utf8(body) else {
        return "bytes";
    };
    if text
        .chars()
        .any(|c| c.is_control() && !matches!(c, '\t' | '\n' | '\r' | '\x0c'))
    {
        return "bytes";
    }
    let trimmed = text.trim();
    if trimmed.starts_with(['{', '['])
        && serde_json::from_str::<serde::de::IgnoredAny>(trimmed).is_ok()
    {
        return "json";
    }
    if trimmed.starts_with("<?xml") || looks_like_xml(trimmed) {
        return "xml";
    }
    if trimmed.starts_with("%YAML")
        || trimmed == "---"
        || trimmed.starts_with("---\n")
        || trimmed.starts_with("---\r\n")
        || trimmed.starts_with("--- ")
    {
        return "yaml";
    }
    "txt"
}

fn looks_like_xml(text: &str) -> bool {
    let mut chars = text.chars();
    chars.next() == Some('<')
        && chars
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '!' || c == '_')
        && text.ends_with('>')
}

/// The file extension (without a dot) for the raw data of an AudioClip, from
/// its magic number: `fsb` (FMOD sound bank), `ogg`, `wav`, `m4a`, or `bin`.
#[must_use]
pub fn sniff_audio_extension(bytes: &[u8]) -> &'static str {
    match bytes {
        [b'F', b'S', b'B', ..] => "fsb",
        [b'O', b'g', b'g', b'S', ..] => "ogg",
        _ if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WAVE") => "wav",
        [_, _, _, _, b'f', b't', b'y', b'p', ..] => "m4a",
        _ => "bin",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_kinds() {
        assert_eq!(sniff_text_extension(br#"{"a": [1, 2]}"#), "json");
        assert_eq!(sniff_text_extension(b"\xEF\xBB\xBF  [1,2,3]\n"), "json");
        // Braces alone do not make JSON.
        assert_eq!(sniff_text_extension(b"{ not json"), "txt");
        assert_eq!(sniff_text_extension(b"<?xml version=\"1.0\"?><a/>"), "xml");
        assert_eq!(sniff_text_extension(b"<root><a>1</a></root>\n"), "xml");
        assert_eq!(sniff_text_extension(b"<3 you"), "txt");
        assert_eq!(sniff_text_extension(b"%YAML 1.1\n---\na: 1"), "yaml");
        assert_eq!(sniff_text_extension(b"---\nkey: value\n"), "yaml");
        assert_eq!(sniff_text_extension(b"hello\tworld\r\n"), "txt");
        assert_eq!(sniff_text_extension(b""), "txt");
        assert_eq!(sniff_text_extension(&[0xFF, 0xFE, b'h', 0]), "txt");
        assert_eq!(sniff_text_extension(&[0, 1, 2, 3]), "bytes");
        assert_eq!(sniff_text_extension(&[0xC3, 0x28]), "bytes");
        assert_eq!(sniff_text_extension(b"abc\x01def"), "bytes");
    }

    #[test]
    fn audio_kinds() {
        assert_eq!(sniff_audio_extension(b"FSB5\0\0\0\0"), "fsb");
        assert_eq!(sniff_audio_extension(b"OggS\0\x02"), "ogg");
        assert_eq!(sniff_audio_extension(b"RIFF\x24\0\0\0WAVEfmt "), "wav");
        assert_eq!(sniff_audio_extension(b"\0\0\0\x20ftypM4A "), "m4a");
        assert_eq!(sniff_audio_extension(b"xx"), "bin");
    }
}
