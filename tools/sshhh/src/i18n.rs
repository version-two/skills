use std::collections::BTreeMap;
use std::fmt::Display;

use crate::error::Error;

pub const LANG_VAR: &str = "SSHHH_LANG";
const SUPPORTED: [(&str, &str); 1] = [("en", include_str!("../locales/en.json"))];

/// Human-facing text of the interactive commands. Machine output (`--json`, error codes) is not
/// translated.
pub struct Catalog {
    messages: BTreeMap<String, String>,
}

impl Catalog {
    /// `None` selects English. An unsupported language is an error, not a silent substitute.
    pub fn load(language: Option<&str>) -> Result<Catalog, Error> {
        let wanted = language.map(|l| l.trim().to_ascii_lowercase()).filter(|l| !l.is_empty()).unwrap_or_else(|| "en".to_string());
        let Some((_, source)) = SUPPORTED.iter().find(|(code, _)| *code == wanted) else {
            let available: Vec<&str> = SUPPORTED.iter().map(|(code, _)| *code).collect();
            return Err(Error::Usage(format!("{LANG_VAR}={wanted} is not available; available: {}", available.join(", "))));
        };
        let messages = serde_json::from_str(source).map_err(|e| Error::Io(format!("the {wanted} message catalog is malformed: {e}")))?;
        Ok(Catalog { messages })
    }

    pub fn t(&self, key: &str, args: &[(&str, &dyn Display)]) -> String {
        let Some(template) = self.messages.get(key) else { return format!("[{key}]") };
        let mut text = template.clone();
        for (name, value) in args {
            text = text.replace(&format!("{{{name}}}"), &value.to_string());
        }
        text
    }

    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.messages.keys().map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholders_are_filled_and_unknown_languages_are_refused() {
        let catalog = Catalog::load(None).unwrap();
        assert!(catalog.keys().count() > 0);
        assert_eq!(catalog.t("no.such.key", &[]), "[no.such.key]");
        assert_eq!(Catalog::load(Some("EN")).unwrap().keys().count(), catalog.keys().count());
        assert_eq!(Catalog::load(Some("xx")).err().unwrap().code(), "usage");
    }

    #[test]
    fn every_key_used_in_the_binary_exists_and_every_key_is_used() {
        let catalog = Catalog::load(None).unwrap();
        let sources = [
            include_str!("../src/main.rs"),
            include_str!("../src/cli/mod.rs"),
            include_str!("../src/cli/admin.rs"),
            include_str!("../src/cli/run.rs"),
        ];
        let known: std::collections::BTreeSet<String> = catalog.keys().map(str::to_string).collect();
        let areas: std::collections::BTreeSet<&str> = known.iter().filter_map(|k| k.split_once('.').map(|(area, _)| area)).collect();
        let mut used = std::collections::BTreeSet::new();
        for source in sources {
            for piece in source.split('"').skip(1).step_by(2) {
                let is_key = piece.split_once('.').is_some_and(|(area, rest)| {
                    areas.contains(area) && !rest.is_empty() && rest.chars().all(|c| c.is_ascii_lowercase() || c == '_')
                });
                if is_key {
                    used.insert(piece.to_string());
                }
            }
        }
        assert_eq!(used.difference(&known).collect::<Vec<_>>(), Vec::<&String>::new(), "keys used but missing from the catalog");
        assert_eq!(known.difference(&used).collect::<Vec<_>>(), Vec::<&String>::new(), "catalog keys that nothing uses");
    }
}
