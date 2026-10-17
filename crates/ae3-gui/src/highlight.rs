//! Basic syntax highlighting for text Assets: JSON, XML and YAML.
//!
//! The tokenizers are forgiving: they never fail, and every byte of the
//! input ends up in exactly one token, in order, so the tokens can be laid
//! out directly.

use std::ops::Range;

/// A text Asset's language, from its sniffed extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    Json,
    Xml,
    Yaml,
    Plain,
}

impl Language {
    #[must_use]
    pub fn from_extension(ext: &str) -> Self {
        match ext {
            "json" => Self::Json,
            "xml" => Self::Xml,
            "yaml" | "yml" => Self::Yaml,
            _ => Self::Plain,
        }
    }
}

/// What a token is, for choosing its colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    Plain,
    /// An object key (JSON) or mapping key (YAML).
    Key,
    String,
    Number,
    /// `true`, `false`, `null` and the like.
    Keyword,
    Punct,
    Comment,
    /// An XML element name.
    Tag,
    /// An XML attribute name.
    Attribute,
}

pub type Token = (Range<usize>, TokenKind);

/// Splits `text` into tokens.
#[must_use]
pub fn tokenize(text: &str, language: Language) -> Vec<Token> {
    let tokens = match language {
        Language::Json => json(text),
        Language::Xml => xml(text),
        Language::Yaml => yaml(text),
        Language::Plain => vec![(0..text.len(), TokenKind::Plain)],
    };
    merge(tokens)
}

/// Joins neighbouring tokens of the same kind and drops empty ones.
fn merge(tokens: Vec<Token>) -> Vec<Token> {
    let mut out: Vec<Token> = Vec::with_capacity(tokens.len());
    for (range, kind) in tokens {
        if range.is_empty() {
            continue;
        }
        match out.last_mut() {
            Some((last, k)) if *k == kind && last.end == range.start => last.end = range.end,
            _ => out.push((range, kind)),
        }
    }
    out
}

/// The end of a quoted string starting at `start` (which holds the quote),
/// honouring backslash escapes; the end of the text if unterminated.
fn quoted_end(bytes: &[u8], start: usize, escapes: bool) -> usize {
    let quote = bytes[start];
    let mut i = start + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if escapes => i += 2,
            b if b == quote => return i + 1,
            _ => i += 1,
        }
    }
    bytes.len()
}

fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'+' | b'.')
}

fn json(text: &str) -> Vec<Token> {
    let bytes = text.as_bytes();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let start = i;
        let kind = match bytes[i] {
            b'"' => {
                i = quoted_end(bytes, i, true);
                let next = bytes[i..].iter().find(|b| !b.is_ascii_whitespace());
                if next == Some(&b':') {
                    TokenKind::Key
                } else {
                    TokenKind::String
                }
            }
            b'{' | b'}' | b'[' | b']' | b',' | b':' => {
                i += 1;
                TokenKind::Punct
            }
            b if b == b'-' || b.is_ascii_digit() => {
                while i < bytes.len() && is_word_byte(bytes[i]) {
                    i += 1;
                }
                TokenKind::Number
            }
            b if b.is_ascii_alphabetic() => {
                while i < bytes.len() && bytes[i].is_ascii_alphanumeric() {
                    i += 1;
                }
                if matches!(&text[start..i], "true" | "false" | "null") {
                    TokenKind::Keyword
                } else {
                    TokenKind::Plain
                }
            }
            _ => {
                // Whitespace or anything unexpected: one character (merged
                // with its neighbours later).
                i += text[i..].chars().next().map_or(1, char::len_utf8);
                TokenKind::Plain
            }
        };
        tokens.push((start..i, kind));
    }
    tokens
}

fn xml(text: &str) -> Vec<Token> {
    let bytes = text.as_bytes();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if text[i..].starts_with("<!--") {
            let end = text[i + 4..]
                .find("-->")
                .map_or(bytes.len(), |e| i + 4 + e + 3);
            tokens.push((i..end, TokenKind::Comment));
            i = end;
        } else if bytes[i] == b'<' {
            i = xml_tag(text, i, &mut tokens);
        } else {
            let end = text[i..].find('<').map_or(bytes.len(), |e| i + e);
            tokens.push((i..end, TokenKind::Plain));
            i = end;
        }
    }
    tokens
}

/// Tokenizes one tag starting at `start` (`<`); returns the index after it.
fn xml_tag(text: &str, start: usize, tokens: &mut Vec<Token>) -> usize {
    let bytes = text.as_bytes();
    let mut i = start + 1;
    while i < bytes.len() && matches!(bytes[i], b'/' | b'?' | b'!') {
        i += 1;
    }
    tokens.push((start..i, TokenKind::Punct));
    let name_start = i;
    while i < bytes.len() && !bytes[i].is_ascii_whitespace() && !matches!(bytes[i], b'>' | b'/') {
        i += 1;
    }
    tokens.push((name_start..i, TokenKind::Tag));
    while i < bytes.len() {
        let s = i;
        match bytes[i] {
            b'>' => {
                tokens.push((s..i + 1, TokenKind::Punct));
                return i + 1;
            }
            b'"' | b'\'' => {
                i = quoted_end(bytes, i, false);
                tokens.push((s..i, TokenKind::String));
            }
            b'=' | b'/' | b'?' => {
                i += 1;
                tokens.push((s..i, TokenKind::Punct));
            }
            b if b.is_ascii_whitespace() => {
                while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                    i += 1;
                }
                tokens.push((s..i, TokenKind::Plain));
            }
            _ => {
                while i < bytes.len()
                    && !bytes[i].is_ascii_whitespace()
                    && !matches!(bytes[i], b'=' | b'>' | b'/' | b'"' | b'\'')
                {
                    i += 1;
                }
                tokens.push((s..i, TokenKind::Attribute));
            }
        }
    }
    i
}

fn yaml(text: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        yaml_line(line, offset, &mut tokens);
        offset += line.len();
    }
    tokens
}

fn yaml_scalar_kind(value: &str) -> TokenKind {
    let v = value.trim();
    if v.is_empty() {
        TokenKind::Plain
    } else if matches!(
        v,
        "true" | "false" | "null" | "~" | "yes" | "no" | "True" | "False" | "Null"
    ) {
        TokenKind::Keyword
    } else if v.parse::<f64>().is_ok() {
        TokenKind::Number
    } else {
        TokenKind::Plain
    }
}

fn yaml_line(line: &str, base: usize, tokens: &mut Vec<Token>) {
    let bytes = line.as_bytes();
    let at = |r: Range<usize>| base + r.start..base + r.end;
    let indent = bytes.iter().take_while(|b| **b == b' ').count();
    tokens.push((at(0..indent), TokenKind::Plain));
    let mut i = indent;
    let rest = &line[i..];
    if rest.starts_with('#') {
        tokens.push((at(i..line.len()), TokenKind::Comment));
        return;
    }
    if rest.starts_with("---") || rest.starts_with("...") {
        tokens.push((at(i..i + 3), TokenKind::Punct));
        i += 3;
    }
    // List markers.
    while line[i..].starts_with("- ") {
        tokens.push((at(i..i + 2), TokenKind::Punct));
        i += 2;
    }
    // A mapping key: up to the first ": " (or ':' at the end of the line)
    // outside quotes.
    let body = &line[i..];
    if !body.starts_with(['"', '\'']) {
        let key_end = body
            .match_indices(':')
            .map(|(p, _)| p)
            .find(|&p| body[p + 1..].is_empty() || body[p + 1..].starts_with([' ', '\n', '\r']));
        if let Some(p) = key_end {
            tokens.push((at(i..i + p), TokenKind::Key));
            tokens.push((at(i + p..i + p + 1), TokenKind::Punct));
            i += p + 1;
        }
    }
    // The value, possibly quoted, possibly followed by a comment.
    let value = &line[i..];
    let lead = value.len() - value.trim_start().len();
    tokens.push((at(i..i + lead), TokenKind::Plain));
    i += lead;
    let bytes_rest = &bytes[i..];
    if bytes_rest
        .first()
        .is_some_and(|b| matches!(b, b'"' | b'\''))
    {
        let end = i + quoted_end(bytes_rest, 0, bytes_rest[0] == b'"');
        tokens.push((at(i..end), TokenKind::String));
        i = end;
    }
    let tail = &line[i..];
    let comment = tail
        .match_indices(" #")
        .next()
        .map(|(p, _)| p + 1)
        .or_else(|| tail.starts_with('#').then_some(0));
    let (value_end, comment_start) = comment.map_or((line.len(), None), |p| (i + p, Some(i + p)));
    tokens.push((at(i..value_end), yaml_scalar_kind(&line[i..value_end])));
    if let Some(c) = comment_start {
        tokens.push((at(c..line.len()), TokenKind::Comment));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tokens cover the text exactly, in order.
    fn check_cover(text: &str, tokens: &[Token]) {
        let mut pos = 0;
        for (range, _) in tokens {
            assert_eq!(range.start, pos, "gap or overlap at {pos} in {text:?}");
            assert!(text.is_char_boundary(range.start) && text.is_char_boundary(range.end));
            pos = range.end;
        }
        assert_eq!(pos, text.len());
    }

    fn kinds_of<'t>(text: &'t str, tokens: &[Token], kind: TokenKind) -> Vec<&'t str> {
        tokens
            .iter()
            .filter(|(_, k)| *k == kind)
            .map(|(r, _)| &text[r.clone()])
            .collect()
    }

    #[test]
    fn json_keys_strings_numbers_keywords() {
        let text =
            "{\"name\": \"Bossk \\\"the\\\" hunter\", \"lvl\": -8.5e2, \"ok\": [true, null]}";
        let tokens = tokenize(text, Language::Json);
        check_cover(text, &tokens);
        assert_eq!(
            kinds_of(text, &tokens, TokenKind::Key),
            ["\"name\"", "\"lvl\"", "\"ok\""]
        );
        assert_eq!(
            kinds_of(text, &tokens, TokenKind::String),
            ["\"Bossk \\\"the\\\" hunter\""]
        );
        assert_eq!(kinds_of(text, &tokens, TokenKind::Number), ["-8.5e2"]);
        assert_eq!(
            kinds_of(text, &tokens, TokenKind::Keyword),
            ["true", "null"]
        );
    }

    #[test]
    fn json_survives_garbage_and_unicode() {
        for text in ["{\"a\": \"unterminated", "é ü 漢字 {", "\"\\", "", "@@@ :"] {
            check_cover(text, &tokenize(text, Language::Json));
        }
    }

    #[test]
    fn xml_tags_attributes_comments() {
        let text =
            "<?xml version=\"1.0\"?>\n<!-- note -->\n<unit id='bossk' tier=\"7\">Text</unit>";
        let tokens = tokenize(text, Language::Xml);
        check_cover(text, &tokens);
        assert_eq!(
            kinds_of(text, &tokens, TokenKind::Tag),
            ["xml", "unit", "unit"]
        );
        assert_eq!(
            kinds_of(text, &tokens, TokenKind::Attribute),
            ["version", "id", "tier"]
        );
        assert_eq!(
            kinds_of(text, &tokens, TokenKind::Comment),
            ["<!-- note -->"]
        );
        assert_eq!(
            kinds_of(text, &tokens, TokenKind::String),
            ["\"1.0\"", "'bossk'", "\"7\""]
        );
        for bad in ["<unclosed attr=\"x", "<!-- open", "<", "a < b > c"] {
            check_cover(bad, &tokenize(bad, Language::Xml));
        }
    }

    #[test]
    fn yaml_keys_values_comments() {
        let text = "# header\n---\nunit: bossk\nstats:\n  - speed: 120 # fast\n  - name: \"Trandoshan: hunter\"\n  ready: true\nurl: http://x\n";
        let tokens = tokenize(text, Language::Yaml);
        check_cover(text, &tokens);
        assert_eq!(
            kinds_of(text, &tokens, TokenKind::Key),
            ["unit", "stats", "speed", "name", "ready", "url"]
        );
        assert_eq!(kinds_of(text, &tokens, TokenKind::Number), ["120 "]);
        assert_eq!(kinds_of(text, &tokens, TokenKind::Keyword), ["true\n"]);
        assert_eq!(
            kinds_of(text, &tokens, TokenKind::String),
            ["\"Trandoshan: hunter\""]
        );
        assert_eq!(
            kinds_of(text, &tokens, TokenKind::Comment),
            ["# header\n", "# fast\n"]
        );
        for bad in ["key: 'open", ":", "- - -", "a:b", "é: ü"] {
            check_cover(bad, &tokenize(bad, Language::Yaml));
        }
    }

    #[test]
    fn plain_is_one_token() {
        assert_eq!(
            tokenize("abc", Language::Plain),
            vec![(0..3, TokenKind::Plain)]
        );
        assert_eq!(Language::from_extension("yml"), Language::Yaml);
        assert_eq!(Language::from_extension("bytes"), Language::Plain);
    }
}
