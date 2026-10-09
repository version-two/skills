use std::collections::BTreeMap;
use std::path::Path;

use crate::error::Error;

pub fn parse(text: &str, path: &Path) -> Result<BTreeMap<String, String>, Error> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let fail = |line: usize, reason: &'static str| Error::EnvParse { path: path.to_path_buf(), line, reason };
    let mut out = BTreeMap::new();
    let lines: Vec<&str> = text.split('\n').collect();
    let mut i = 0;
    while i < lines.len() {
        let lineno = i + 1;
        let line = lines[i].trim_end_matches('\r').trim_start();
        i += 1;
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").map_or(line, str::trim_start);
        let (key, rest) = line.split_once('=').ok_or_else(|| fail(lineno, "expected KEY=VALUE"))?;
        let key = key.trim();
        if !valid_key(key) {
            return Err(fail(lineno, "invalid key name"));
        }
        let rest = rest.trim_start();
        let value = match rest.chars().next() {
            Some('"') => {
                let mut body = rest[1..].to_string();
                let (value, tail) = loop {
                    if let Some(done) = take_double_quoted(&body) {
                        break done;
                    }
                    if i >= lines.len() {
                        return Err(fail(lineno, "unterminated double quote"));
                    }
                    body.push('\n');
                    body.push_str(lines[i].trim_end_matches('\r'));
                    i += 1;
                };
                check_tail(&tail).map_err(|r| fail(lineno, r))?;
                value
            }
            Some('\'') => {
                let mut body = rest[1..].to_string();
                let end = loop {
                    if let Some(end) = body.find('\'') {
                        break end;
                    }
                    if i >= lines.len() {
                        return Err(fail(lineno, "unterminated single quote"));
                    }
                    body.push('\n');
                    body.push_str(lines[i].trim_end_matches('\r'));
                    i += 1;
                };
                check_tail(&body[end + 1..]).map_err(|r| fail(lineno, r))?;
                body[..end].to_string()
            }
            _ => strip_inline_comment(rest).trim_end().to_string(),
        };
        out.insert(key.to_ascii_uppercase(), value);
    }
    Ok(out)
}

fn valid_key(key: &str) -> bool {
    let mut chars = key.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn take_double_quoted(body: &str) -> Option<(String, String)> {
    let mut value = String::new();
    let mut chars = body.char_indices();
    while let Some((idx, c)) = chars.next() {
        match c {
            '"' => return Some((value, body[idx + 1..].to_string())),
            '\\' => match chars.next() {
                Some((_, 'n')) => value.push('\n'),
                Some((_, 'r')) => value.push('\r'),
                Some((_, 't')) => value.push('\t'),
                Some((_, '"')) => value.push('"'),
                Some((_, '\\')) => value.push('\\'),
                Some((_, other)) => {
                    value.push('\\');
                    value.push(other);
                }
                None => value.push('\\'),
            },
            other => value.push(other),
        }
    }
    None
}

fn check_tail(tail: &str) -> Result<(), &'static str> {
    let tail = tail.trim();
    if tail.is_empty() || tail.starts_with('#') { Ok(()) } else { Err("unexpected text after closing quote") }
}

fn strip_inline_comment(value: &str) -> &str {
    let bytes = value.as_bytes();
    for (idx, &b) in bytes.iter().enumerate() {
        if b == b'#' && idx > 0 && bytes[idx - 1].is_ascii_whitespace() {
            return &value[..idx];
        }
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(text: &str) -> BTreeMap<String, String> {
        parse(text, Path::new("test.env")).unwrap()
    }

    #[test]
    fn plain_pairs_and_comments() {
        let m = p("# c\n\nzeus_host=1.2.3.4\nexport ZEUS_PORT = 2222 # note\nZEUS_PASS=ab#cd\n");
        assert_eq!(m["ZEUS_HOST"], "1.2.3.4");
        assert_eq!(m["ZEUS_PORT"], "2222");
        assert_eq!(m["ZEUS_PASS"], "ab#cd");
    }

    #[test]
    fn quotes_and_escapes() {
        let m = p("A=\"x y\\n\\\"q\\\" #not\" # c\nB='lit \\n #x'\nC=\"\"\n");
        assert_eq!(m["A"], "x y\n\"q\" #not");
        assert_eq!(m["B"], "lit \\n #x");
        assert_eq!(m["C"], "");
    }

    #[test]
    fn multiline_quoted_value_and_crlf() {
        let m = p("\u{feff}K=\"line1\r\nline2\"\r\nX=1\r\n");
        assert_eq!(m["K"], "line1\nline2");
        assert_eq!(m["X"], "1");
    }

    #[test]
    fn errors_name_the_line_never_the_content() {
        let e = parse("A=1\nsecret-without-equals\n", Path::new("t.env")).unwrap_err();
        let msg = e.to_string();
        assert!(msg.contains("line 2"));
        assert!(!msg.contains("secret-without-equals"));
        assert!(parse("A=\"open", Path::new("t.env")).is_err());
        assert!(parse("A=\"x\" junk", Path::new("t.env")).is_err());
        assert!(parse("1A=x", Path::new("t.env")).is_err());
    }
}
