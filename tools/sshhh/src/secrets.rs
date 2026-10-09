use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use tokio::process::Command;
use tokio::sync::{Mutex, OnceCell};
use zeroize::{Zeroize, Zeroizing};

use crate::config::Settings;
use crate::error::Error;

const IMPLEMENTED: [&str; 3] = ["bw", "env", "file"];
const DEFERRED: [&str; 14] = [
    "bws", "op", "vault", "az", "aws-sm", "cred", "gcloud", "pass", "gopass", "keepassxc", "doppler", "infisical", "sops", "age",
];
const STANDARD_FIELDS: [&str; 5] = ["password", "username", "totp", "notes", "uri"];
const BW_TIMEOUT: Duration = Duration::from_secs(30);
const BW_CLOUD: &str = "https://bitwarden.com";

#[derive(Debug, PartialEq, Eq)]
pub enum Target {
    Env(String),
    File(String),
    Bitwarden { item: String, field: Option<String> },
}

#[derive(Debug, PartialEq, Eq)]
pub struct Reference {
    pub raw: String,
    pub target: Target,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    Line,
    Whole,
}

fn unavailable(provider: &'static str, reference: &str, reason: &'static str, detail: impl Into<String>) -> Error {
    Error::SecretUnavailable { provider, reference: reference.to_string(), reason, detail: detail.into() }
}

fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let pair = bytes.get(i + 1..i + 3)?;
            if !pair.iter().all(u8::is_ascii_hexdigit) {
                return None;
            }
            out.push(u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// `Ok(None)` means the value is a literal. Only known scheme names count as references, so
/// values such as `C:\key` or `https://...` stay literal.
pub fn parse_reference(value: &str) -> Result<Option<Reference>, Error> {
    let Some((scheme, rest)) = value.split_once("://") else { return Ok(None) };
    let scheme = scheme.to_ascii_lowercase();
    if DEFERRED.contains(&scheme.as_str()) {
        return Err(unavailable("sshhh", value, "not_implemented", format!("the {scheme}:// provider is not available in this version")));
    }
    if !IMPLEMENTED.contains(&scheme.as_str()) {
        return Ok(None);
    }
    let bad = |detail: &str| unavailable("sshhh", value, "bad_reference", detail.to_string());
    let target = match scheme.as_str() {
        "env" => {
            if rest.is_empty() {
                return Err(bad("expected env://NAME"));
            }
            Target::Env(rest.to_string())
        }
        "file" => {
            if rest.is_empty() {
                return Err(bad("expected file://PATH"));
            }
            Target::File(rest.to_string())
        }
        _ => {
            let (item_raw, field_raw) = match rest.rfind('/') {
                Some(i) => (&rest[..i], Some(&rest[i + 1..])),
                None => (rest, None),
            };
            let decode = |part: &str| percent_decode(part).ok_or_else(|| bad("invalid percent-encoding"));
            let item = decode(item_raw)?;
            if item.is_empty() {
                return Err(bad("expected bw://ITEM[/FIELD]"));
            }
            if item.starts_with('-') {
                return Err(bad("an item name must not start with '-'"));
            }
            let field = match field_raw {
                Some("") => return Err(bad("empty field name after '/'")),
                Some(f) => Some(decode(f)?),
                None => None,
            };
            Target::Bitwarden { item, field }
        }
    };
    Ok(Some(Reference { raw: value.to_string(), target }))
}

#[derive(Deserialize)]
struct RawItem {
    login: Option<RawLogin>,
    notes: Option<SecretString>,
    fields: Option<Vec<RawField>>,
}

#[derive(Deserialize)]
struct RawLogin {
    username: Option<SecretString>,
    password: Option<SecretString>,
    uris: Option<Vec<RawUri>>,
}

#[derive(Deserialize)]
struct RawUri {
    uri: Option<SecretString>,
}

#[derive(Deserialize)]
struct RawField {
    name: Option<String>,
    value: Option<SecretString>,
}

pub struct BwItem {
    label: String,
    username: Option<SecretString>,
    password: Option<SecretString>,
    notes: Option<SecretString>,
    uri: Option<SecretString>,
    fields: Vec<(String, Option<SecretString>)>,
}

fn copy(secret: &Option<SecretString>) -> Option<SecretString> {
    secret.as_ref().filter(|s| !s.expose_secret().is_empty()).map(|s| SecretString::from(s.expose_secret().to_owned()))
}

impl BwItem {
    fn parse(label: &str, json: &[u8]) -> Result<BwItem, Error> {
        let raw: RawItem = serde_json::from_slice(json)
            .map_err(|_| unavailable("bw", label, "bw_bad_output", "`bw get item` did not return the expected JSON"))?;
        let (username, password, uri) = match raw.login {
            Some(login) => (login.username, login.password, login.uris.and_then(|u| u.into_iter().next()).and_then(|u| u.uri)),
            None => (None, None, None),
        };
        let fields = raw.fields.unwrap_or_default().into_iter().filter_map(|f| f.name.map(|n| (n, f.value))).collect();
        Ok(BwItem { label: label.to_string(), username, password, notes: raw.notes, uri, fields })
    }

    fn standard(&self, name: &str) -> Option<SecretString> {
        match name {
            "password" => copy(&self.password),
            "username" => copy(&self.username),
            "notes" => copy(&self.notes),
            "uri" => copy(&self.uri),
            _ => None,
        }
    }

    /// `Ok(None)`: the item has no such field. A name carried twice is an error rather than a guess.
    pub fn custom(&self, name: &str) -> Result<Option<SecretString>, Error> {
        let mut found = self.fields.iter().filter(|(n, _)| n.eq_ignore_ascii_case(name));
        let Some((_, first)) = found.next() else { return Ok(None) };
        if found.next().is_some() {
            return Err(unavailable("bw", &self.label, "ambiguous", format!("the item has more than one custom field named {name}")));
        }
        Ok(copy(first))
    }
}

struct Bitwarden {
    bin: String,
    appdata: Option<PathBuf>,
    expected_server: Option<String>,
    session: Option<SecretString>,
    server_checked: OnceCell<()>,
    items: Mutex<HashMap<String, Arc<BwItem>>>,
}

fn classify(text: &str) -> &'static str {
    let t = text.to_ascii_lowercase();
    if t.contains("vault is locked") {
        "vault_locked"
    } else if t.contains("not logged in") {
        "not_logged_in"
    } else if t.contains("more than one result") {
        "ambiguous"
    } else if t.contains("not found") {
        "not_found"
    } else {
        "bw_failed"
    }
}

fn describe(reason: &str, text: &str) -> String {
    match reason {
        "vault_locked" => "run `sshhh unlock bw` or export BW_SESSION".to_string(),
        "not_logged_in" => "run `bw login` (and `bw config server` first for a self-hosted server)".to_string(),
        _ => text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("no message").chars().take(200).collect(),
    }
}

fn normalize_server(url: &str) -> String {
    url.trim().trim_end_matches('/').to_ascii_lowercase()
}

impl Bitwarden {
    async fn run(&self, reference: &str, args: &[&str]) -> Result<Zeroizing<Vec<u8>>, Error> {
        let mut cmd = Command::new(&self.bin);
        cmd.args(args)
            .arg("--nointeraction")
            .env("BW_NOINTERACTION", "true")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(dir) = &self.appdata {
            cmd.env("BITWARDENCLI_APPDATA_DIR", dir);
        }
        if let Some(session) = &self.session {
            cmd.env("BW_SESSION", session.expose_secret());
        }
        #[cfg(windows)]
        cmd.creation_flags(0x0800_0000);

        let output = match tokio::time::timeout(BW_TIMEOUT, cmd.output()).await {
            Err(_) => return Err(unavailable("bw", reference, "bw_timeout", format!("bw did not answer within {} s", BW_TIMEOUT.as_secs()))),
            Ok(Err(e)) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(unavailable("bw", reference, "bw_not_installed", format!("cannot run '{}'; install the native Bitwarden CLI or set BW_BIN", self.bin)));
            }
            Ok(Err(e)) => return Err(unavailable("bw", reference, "bw_spawn_failed", e.kind().to_string())),
            Ok(Ok(output)) => output,
        };
        let stdout = Zeroizing::new(output.stdout);
        if output.status.success() {
            return Ok(stdout);
        }
        let mut text = String::from_utf8_lossy(&output.stderr).into_owned();
        text.push('\n');
        text.push_str(&String::from_utf8_lossy(&stdout));
        let reason = classify(&text);
        let detail = describe(reason, &text);
        text.zeroize();
        Err(unavailable("bw", reference, reason, detail))
    }

    async fn check_server(&self, reference: &str) -> Result<(), Error> {
        let Some(expected) = &self.expected_server else { return Ok(()) };
        self.server_checked
            .get_or_try_init(|| async {
                let out = self.run(reference, &["status"]).await?;
                let status: serde_json::Value = serde_json::from_slice(&out)
                    .map_err(|_| unavailable("bw", reference, "bw_bad_output", "`bw status` did not return JSON"))?;
                let actual = status.get("serverUrl").and_then(|v| v.as_str()).unwrap_or(BW_CLOUD);
                if normalize_server(actual) != normalize_server(expected) {
                    return Err(unavailable("bw", reference, "bw_wrong_server", format!("bw is connected to {actual}, expected {expected}")));
                }
                Ok(())
            })
            .await
            .map(|_| ())
    }

    async fn item(&self, item: &str, reference: &str) -> Result<Arc<BwItem>, Error> {
        let mut cache = self.items.lock().await;
        if let Some(hit) = cache.get(item) {
            return Ok(hit.clone());
        }
        self.check_server(reference).await?;
        let out = self.run(reference, &["get", "item", item]).await?;
        let parsed = Arc::new(BwItem::parse(reference, &out)?);
        cache.insert(item.to_string(), parsed.clone());
        Ok(parsed)
    }

    async fn totp(&self, item: &str, reference: &str) -> Result<SecretString, Error> {
        self.check_server(reference).await?;
        let out = self.run(reference, &["get", "totp", item, "--raw"]).await?;
        let code = String::from_utf8_lossy(&out).trim().to_string();
        if code.is_empty() {
            return Err(unavailable("bw", reference, "not_found", "the item has no TOTP seed"));
        }
        Ok(SecretString::from(code))
    }
}

/// Runs `bw unlock --raw` on the caller's terminal so the master password is typed there, and
/// returns the session key it prints.
pub async fn unlock_bitwarden(settings: &Settings) -> Result<SecretString, Error> {
    let bin = settings.bw_bin.clone().unwrap_or_else(|| "bw".to_string());
    let mut cmd = Command::new(&bin);
    cmd.args(["unlock", "--raw"]).stdin(Stdio::inherit()).stdout(Stdio::piped()).stderr(Stdio::inherit()).kill_on_drop(true);
    if let Some(dir) = &settings.bw_appdata {
        cmd.env("BITWARDENCLI_APPDATA_DIR", dir);
    }
    let output = match cmd.output().await {
        Ok(output) => output,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(unavailable("bw", "bw://", "bw_not_installed", format!("cannot run '{bin}'; install the native Bitwarden CLI or set BW_BIN")));
        }
        Err(e) => return Err(unavailable("bw", "bw://", "bw_spawn_failed", e.kind().to_string())),
    };
    let stdout = Zeroizing::new(output.stdout);
    if !output.status.success() {
        return Err(unavailable("bw", "bw://", "bw_failed", "`bw unlock` did not succeed"));
    }
    let key = String::from_utf8_lossy(&stdout).trim().to_string();
    if key.is_empty() {
        return Err(unavailable("bw", "bw://", "bw_bad_output", "`bw unlock --raw` printed no session key"));
    }
    Ok(SecretString::from(key))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BwStatus {
    pub version: String,
    pub state: String,
    pub server_url: String,
}

impl BwStatus {
    pub fn on_server(&self, expected: &str) -> bool {
        normalize_server(&self.server_url) == normalize_server(expected)
    }
}

pub struct Secrets {
    env: BTreeMap<String, String>,
    home: Option<PathBuf>,
    bw: Bitwarden,
}

impl Secrets {
    pub fn new(settings: &Settings, env: BTreeMap<String, String>, home: Option<PathBuf>) -> Secrets {
        let bw = Bitwarden {
            bin: settings.bw_bin.clone().unwrap_or_else(|| "bw".to_string()),
            appdata: settings.bw_appdata.clone(),
            expected_server: settings.bw_server.clone(),
            session: env.get("BW_SESSION").filter(|s| !s.is_empty()).map(|s| SecretString::from(s.clone())),
            server_checked: OnceCell::new(),
            items: Mutex::new(HashMap::new()),
        };
        Secrets { env, home, bw }
    }

    pub fn set_bw_session(&mut self, session: SecretString) {
        self.bw.session = Some(session);
    }

    pub async fn bw_status(&self) -> Result<BwStatus, Error> {
        let reference = "bw://";
        let version = self.bw.run(reference, &["--version"]).await?;
        let status = self.bw.run(reference, &["status"]).await?;
        let status: serde_json::Value =
            serde_json::from_slice(&status).map_err(|_| unavailable("bw", reference, "bw_bad_output", "`bw status` did not return JSON"))?;
        Ok(BwStatus {
            version: String::from_utf8_lossy(&version).trim().to_string(),
            state: status.get("status").and_then(|v| v.as_str()).unwrap_or("unknown").to_string(),
            server_url: status.get("serverUrl").and_then(|v| v.as_str()).unwrap_or(BW_CLOUD).to_string(),
        })
    }

    pub async fn bw_item(&self, reference: &str, item: &str) -> Result<Arc<BwItem>, Error> {
        self.bw.item(item, reference).await
    }

    pub async fn fetch(&self, reference: &Reference, shape: Shape) -> Result<SecretString, Error> {
        let raw = reference.raw.as_str();
        match &reference.target {
            Target::Env(name) => match self.env.get(name).filter(|v| !v.is_empty()) {
                Some(value) => Ok(SecretString::from(value.clone())),
                None => Err(unavailable("env", raw, "not_set", format!("environment variable {name} is not set or empty"))),
            },
            Target::File(spec) => self.read_file(raw, spec, shape).await,
            Target::Bitwarden { item, field } => self.bw_field(raw, item, field.as_deref()).await,
        }
    }

    fn file_path(&self, spec: &str) -> PathBuf {
        let bytes = spec.as_bytes();
        let drive = bytes.len() >= 3 && bytes[0] == b'/' && bytes[1].is_ascii_alphabetic() && bytes[2] == b':';
        if drive {
            return PathBuf::from(&spec[1..]);
        }
        if let (Some(rest), Some(home)) = (spec.strip_prefix("~/"), &self.home) {
            return home.join(rest);
        }
        PathBuf::from(spec)
    }

    async fn read_file(&self, raw: &str, spec: &str, shape: Shape) -> Result<SecretString, Error> {
        let path = self.file_path(spec);
        let mut text = tokio::fs::read_to_string(&path)
            .await
            .map_err(|e| unavailable("file", raw, "unreadable", format!("{}: {}", path.display(), e.kind())))?;
        let value = match shape {
            Shape::Whole => text,
            Shape::Line => {
                let first = text.lines().next().unwrap_or("").trim_end().to_string();
                text.zeroize();
                first
            }
        };
        if value.trim().is_empty() {
            return Err(unavailable("file", raw, "empty", format!("{} has no content", path.display())));
        }
        Ok(SecretString::from(value))
    }

    async fn bw_field(&self, raw: &str, item: &str, field: Option<&str>) -> Result<SecretString, Error> {
        let field = field.unwrap_or("password");
        let custom_name = field.get(..6).filter(|p| p.eq_ignore_ascii_case("field:")).map(|_| &field[6..]);
        if let Some(name) = custom_name {
            return self.bw_custom(raw, item, name).await;
        }
        let lower = field.to_ascii_lowercase();
        if lower == "totp" {
            return self.bw.totp(item, raw).await;
        }
        if STANDARD_FIELDS.contains(&lower.as_str()) {
            let found = self.bw.item(item, raw).await?;
            return found.standard(&lower).ok_or_else(|| {
                unavailable("bw", raw, "not_found", format!("the item has no {lower}; write field:{field} to read a custom field of that name"))
            });
        }
        self.bw_custom(raw, item, field).await
    }

    async fn bw_custom(&self, raw: &str, item: &str, name: &str) -> Result<SecretString, Error> {
        let found = self.bw.item(item, raw).await?;
        found.custom(name)?.ok_or_else(|| unavailable("bw", raw, "not_found", format!("the item has no custom field named {name}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(value: &str) -> Target {
        parse_reference(value).unwrap().unwrap().target
    }

    #[test]
    fn literals_are_not_references() {
        for v in ["hunter2", "C:\\keys\\id", "https://example.test/x", "~/.ssh/id", "unknown://x", ""] {
            assert!(parse_reference(v).unwrap().is_none(), "{v}");
        }
    }

    #[test]
    fn bitwarden_reference_splits_at_the_last_slash() {
        assert_eq!(target("bw://zeus"), Target::Bitwarden { item: "zeus".into(), field: None });
        assert_eq!(target("bw://zeus/username"), Target::Bitwarden { item: "zeus".into(), field: Some("username".into()) });
        assert_eq!(
            target("bw://prod/web%20box/field:API KEY"),
            Target::Bitwarden { item: "prod/web box".into(), field: Some("field:API KEY".into()) }
        );
        assert_eq!(target("BW://a%2Fb/notes"), Target::Bitwarden { item: "a/b".into(), field: Some("notes".into()) });
    }

    #[test]
    fn malformed_references_are_rejected() {
        for v in ["bw://", "bw:///x", "bw://-x", "bw://x/", "bw://x%zz", "env://", "file://"] {
            assert_eq!(parse_reference(v).unwrap_err().code(), "secret_unavailable", "{v}");
        }
    }

    #[test]
    fn deferred_schemes_say_so_instead_of_passing_as_literals() {
        let e = parse_reference("op://vault/item/password").unwrap_err();
        assert!(e.to_string().contains("not_implemented"), "{e}");
    }

    #[test]
    fn server_urls_compare_without_trailing_slash_or_case() {
        assert_eq!(normalize_server("https://Vault.Example.test/ "), normalize_server("https://vault.example.test"));
    }

    #[test]
    fn bw_messages_are_classified() {
        assert_eq!(classify("Vault is locked."), "vault_locked");
        assert_eq!(classify("You are not logged in."), "not_logged_in");
        assert_eq!(classify("Not found."), "not_found");
        assert_eq!(classify("More than one result was found."), "ambiguous");
        assert_eq!(classify("boom"), "bw_failed");
    }
}
