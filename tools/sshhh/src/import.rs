use std::collections::BTreeSet;
use std::path::Path;

use serde::Serialize;

use crate::config::modern_key;
use crate::dotenv;
use crate::error::Error;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Change {
    pub line: usize,
    pub from: String,
    pub to: String,
}

#[derive(Debug)]
pub struct Plan {
    pub changes: Vec<Change>,
    pub text: String,
}

/// Rewrites legacy key names to the current ones and touches nothing else: values, quoting,
/// comments, order and line endings stay as they were.
pub fn plan(text: &str, path: &Path) -> Result<Plan, Error> {
    let (bom, body) = match text.strip_prefix('\u{feff}') {
        Some(rest) => ("\u{feff}", rest),
        None => ("", text),
    };
    let entries = dotenv::parse_entries(body, path)?;
    let present: BTreeSet<&str> = entries.iter().map(|e| e.key.as_str()).collect();
    let mut changes = Vec::new();
    let mut ranges = Vec::new();
    for entry in &entries {
        let Some(to) = modern_key(&entry.key) else { continue };
        if present.contains(to.as_str()) {
            return Err(Error::Usage(format!(
                "{}: line {}: {} would become {to}, which the file already sets; resolve the duplicate first",
                path.display(),
                entry.line,
                entry.key
            )));
        }
        ranges.push((entry.key_range.clone(), to.clone()));
        changes.push(Change { line: entry.line, from: entry.key.clone(), to });
    }
    let mut out = String::with_capacity(text.len());
    out.push_str(bom);
    let mut cursor = 0;
    for (range, to) in ranges {
        out.push_str(&body[cursor..range.start]);
        out.push_str(&to);
        cursor = range.end;
    }
    out.push_str(&body[cursor..]);
    Ok(Plan { changes, text: out })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(text: &str) -> Result<Plan, Error> {
        plan(text, Path::new("t.env"))
    }

    #[test]
    fn only_key_names_change_and_everything_else_is_byte_identical() {
        let text = "# keep me\r\nZEUS_IP=1.2.3.4 # note\r\nexport ZEUS_PASSWORD = \"a b\"\r\nZEUS_USER=root\r\nDEFAULT_SERVER=zeus\r\n";
        let plan = p(text).unwrap();
        assert_eq!(
            plan.text,
            "# keep me\r\nZEUS_HOST=1.2.3.4 # note\r\nexport ZEUS_PASS = \"a b\"\r\nZEUS_USER=root\r\nDEFAULT=zeus\r\n"
        );
        let lines: Vec<(usize, &str, &str)> = plan.changes.iter().map(|c| (c.line, c.from.as_str(), c.to.as_str())).collect();
        assert_eq!(lines, [(2, "ZEUS_IP", "ZEUS_HOST"), (3, "ZEUS_PASSWORD", "ZEUS_PASS"), (5, "DEFAULT_SERVER", "DEFAULT")]);
    }

    #[test]
    fn text_inside_a_multi_line_value_is_never_a_key() {
        let text = "ZEUS_PASS=\"-----BEGIN-----\nZEUS_IP=not-a-key\n-----END-----\"\nZEUS_CERT=~/id\n";
        let plan = p(text).unwrap();
        assert_eq!(plan.text, "ZEUS_PASS=\"-----BEGIN-----\nZEUS_IP=not-a-key\n-----END-----\"\nZEUS_KEY=~/id\n");
        assert_eq!(plan.changes.len(), 1);
    }

    #[test]
    fn a_rewrite_that_collides_with_an_existing_key_is_refused() {
        let err = p("ZEUS_IP=1.1.1.1\nZEUS_HOST=1.1.1.1\n").unwrap_err();
        assert_eq!(err.code(), "usage");
    }

    #[test]
    fn a_current_file_and_a_bom_survive_untouched() {
        let text = "\u{feff}ZEUS_HOST=h\nZEUS_PASS=p\n";
        let plan = p(text).unwrap();
        assert!(plan.changes.is_empty());
        assert_eq!(plan.text, text);
        assert_eq!(p("\u{feff}ZEUS_IP=h\n").unwrap().text, "\u{feff}ZEUS_HOST=h\n");
    }
}
