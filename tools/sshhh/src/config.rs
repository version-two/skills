use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use secrecy::SecretString;
use serde::Serialize;

use crate::dotenv;
use crate::error::Error;

pub const PROCESS_PREFIX: &str = "SSHHH_";
const DEFAULT_PORT: u16 = 22;
const DEFAULT_USER: &str = "root";
const GLOBAL_KEYS: [&str; 3] = ["DEFAULT", "DEFAULT_SERVER", "DEFAULT_TARGET"];
const DEFAULT_ALIAS: &str = "default";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Escalate {
    Su,
    Sudo,
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AuthMethod {
    Key,
    Password,
    Kbdint,
    Agent,
}

#[derive(Debug)]
pub struct Server {
    pub alias: String,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub user: Option<String>,
    pub pass: Option<SecretString>,
    pub key: Option<String>,
    pub key_pass: Option<SecretString>,
    pub root_user: Option<String>,
    pub root_pass: Option<SecretString>,
    pub sudo_pass: Option<SecretString>,
    pub escalate: Option<Escalate>,
    pub auth: Option<Vec<AuthMethod>>,
    pub vault: Option<String>,
}

impl Server {
    pub fn port(&self) -> u16 {
        self.port.unwrap_or(DEFAULT_PORT)
    }

    pub fn user(&self) -> &str {
        self.user.as_deref().unwrap_or(DEFAULT_USER)
    }

    pub fn root_user(&self) -> &str {
        self.root_user.as_deref().unwrap_or(DEFAULT_USER)
    }
}

/// Tool settings that can launch binaries or pick servers; honoured only from the global
/// file, an explicit `--env` file or the process environment, never from a project `.env`.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Settings {
    pub bw_bin: Option<String>,
    pub bw_appdata: Option<PathBuf>,
    pub bw_server: Option<String>,
}

const SETTING_KEYS: [&str; 3] = ["BW_BIN", "BW_APPDATA", "BW_SERVER"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Warning {
    pub warning: &'static str,
    pub key: String,
    pub use_instead: String,
}

#[derive(Debug)]
pub struct Config {
    pub servers: BTreeMap<String, Server>,
    pub incomplete: Vec<String>,
    pub default_alias: Option<String>,
    pub sources: Vec<PathBuf>,
    pub warnings: Vec<Warning>,
    pub settings: Settings,
}

pub struct LoadOptions {
    pub env_file: Option<PathBuf>,
    pub cwd: PathBuf,
    pub home: Option<PathBuf>,
    pub process_env: BTreeMap<String, String>,
}

struct KeySpec {
    canonical: &'static str,
    names: &'static [&'static str],
}

const KEYS: [KeySpec; 11] = [
    KeySpec { canonical: "HOST", names: &["HOST", "IP", "SSH_HOST"] },
    KeySpec { canonical: "PORT", names: &["PORT", "SSH_PORT"] },
    KeySpec { canonical: "USER", names: &["USER", "SSH_USER"] },
    KeySpec { canonical: "PASS", names: &["PASS", "SSH_PASS", "PASSWORD"] },
    KeySpec { canonical: "KEY", names: &["KEY", "CERT", "SSH_KEY"] },
    KeySpec { canonical: "KEY_PASS", names: &["KEY_PASS", "CERT_PASS"] },
    KeySpec { canonical: "ROOT_USER", names: &["ROOT_USER"] },
    KeySpec { canonical: "ROOT_PASS", names: &["ROOT_PASS"] },
    KeySpec { canonical: "SUDO_PASS", names: &["SUDO_PASS"] },
    KeySpec { canonical: "ESCALATE", names: &["ESCALATE"] },
    KeySpec { canonical: "AUTH", names: &["AUTH"] },
];
const VAULT_KEY: &str = "VAULT";

pub fn normalize_alias(alias: &str) -> String {
    alias.trim().to_ascii_lowercase().replace('-', "_")
}

pub fn process_env_from_os() -> BTreeMap<String, String> {
    std::env::vars().filter(|(k, _)| k.starts_with(PROCESS_PREFIX) || k == "SSH_SERVER").collect()
}

impl Config {
    pub fn load(opts: &LoadOptions) -> Result<Config, Error> {
        let mut merged: BTreeMap<String, String> = BTreeMap::new();
        let mut trusted: BTreeMap<String, String> = BTreeMap::new();
        let mut sources = Vec::new();
        let mut ignored = Vec::new();
        let mut layer = |path: &Path, is_trusted: bool| -> Result<(), Error> {
            let text = std::fs::read_to_string(path)
                .map_err(|e| Error::EnvRead { path: path.to_path_buf(), reason: e.kind().to_string() })?;
            for (key, value) in dotenv::parse(&text, path)? {
                if SETTING_KEYS.contains(&key.as_str()) {
                    if is_trusted {
                        trusted.insert(key, value);
                    } else {
                        ignored.push(key);
                    }
                } else {
                    merged.insert(key, value);
                }
            }
            sources.push(path.to_path_buf());
            Ok(())
        };

        let mut searched = Vec::new();
        if let Some(file) = &opts.env_file {
            layer(file, true)?;
        } else {
            if let Some(home) = &opts.home {
                let global = home.join(".config").join("sshhh").join("servers.env");
                searched.push(global.display().to_string());
                if global.is_file() {
                    layer(&global, true)?;
                }
            }
            let mut dir = Some(opts.cwd.as_path());
            while let Some(d) = dir {
                let plain = d.join(".env");
                let local = d.join(".local").join(".env");
                searched.push(plain.display().to_string());
                searched.push(local.display().to_string());
                if plain.is_file() || local.is_file() {
                    if plain.is_file() {
                        layer(&plain, false)?;
                    }
                    if local.is_file() {
                        layer(&local, false)?;
                    }
                    break;
                }
                dir = d.parent();
            }
        }

        let mut ssh_server = None;
        for (key, value) in &opts.process_env {
            if let Some(stripped) = key.strip_prefix(PROCESS_PREFIX) {
                let stripped = stripped.to_ascii_uppercase();
                if SETTING_KEYS.contains(&stripped.as_str()) {
                    trusted.insert(stripped, value.clone());
                } else {
                    merged.insert(stripped, value.clone());
                }
            } else if key == "SSH_SERVER" && !value.is_empty() {
                ssh_server = Some(value.clone());
            }
        }

        if merged.is_empty() && trusted.is_empty() {
            return Err(Error::NoConfig { searched: searched.join(", ") });
        }
        let mut cfg = build(&merged, sources)?;
        if let Some(alias) = ssh_server {
            cfg.default_alias = Some(normalize_alias(&alias));
        }
        let non_empty = |key: &str| trusted.get(key).filter(|v| !v.is_empty()).cloned();
        cfg.settings = Settings {
            bw_bin: non_empty("BW_BIN"),
            bw_appdata: non_empty("BW_APPDATA").map(PathBuf::from),
            bw_server: non_empty("BW_SERVER"),
        };
        for key in ignored {
            cfg.warnings.push(Warning {
                warning: "ignored_in_project_file",
                use_instead: format!("set SSHHH_{key} in the environment or {key} in ~/.config/sshhh/servers.env"),
                key,
            });
        }
        Ok(cfg)
    }

    pub fn aliases(&self) -> Vec<&str> {
        self.servers.keys().map(String::as_str).collect()
    }

    pub fn select(&self, requested: Option<&str>) -> Result<&Server, Error> {
        let known = || self.aliases().join(", ");
        let wanted = requested.map(normalize_alias).or_else(|| self.default_alias.clone());
        if let Some(alias) = wanted {
            if let Some(server) = self.servers.get(&alias) {
                return Ok(server);
            }
            if self.incomplete.contains(&alias) {
                return Err(Error::MissingHost { alias });
            }
            return Err(Error::UnknownServer { alias, known: known() });
        }
        match self.servers.len() {
            1 => Ok(self.servers.values().next().expect("one server")),
            0 => match self.incomplete.first() {
                Some(alias) => Err(Error::MissingHost { alias: alias.clone() }),
                None => Err(Error::NoConfig { searched: String::new() }),
            },
            _ => Err(Error::NoDefaultServer { known: known() }),
        }
    }
}

struct Claimed<'a> {
    names: Vec<(&'a str, &'a str)>,
}

fn build(map: &BTreeMap<String, String>, sources: Vec<PathBuf>) -> Result<Config, Error> {
    let mut warnings = Vec::new();
    let mut default_alias = None;
    for key in GLOBAL_KEYS {
        if let Some(value) = map.get(key).filter(|v| !v.is_empty()) {
            if key != "DEFAULT" {
                warnings.push(Warning { warning: "deprecated_key", key: key.into(), use_instead: "DEFAULT".into() });
            }
            default_alias = Some(normalize_alias(value));
        }
    }

    // alias -> canonical key -> [(env key name, value)]
    let mut grouped: BTreeMap<String, BTreeMap<&'static str, Claimed>> = BTreeMap::new();
    for (key, value) in map {
        if GLOBAL_KEYS.contains(&key.as_str()) {
            continue;
        }
        let Some((alias, canonical, name)) = classify(key) else { continue };
        if name != canonical {
            let use_instead = format!("{}{canonical}", &key[..key.len() - name.len()]);
            warnings.push(Warning { warning: "deprecated_key", key: key.clone(), use_instead });
        }
        grouped
            .entry(alias)
            .or_default()
            .entry(canonical)
            .or_insert_with(|| Claimed { names: Vec::new() })
            .names
            .push((key.as_str(), value.as_str()));
    }

    let mut servers = BTreeMap::new();
    let mut incomplete = Vec::new();
    for (alias, keys) in &grouped {
        let get = |canonical: &'static str| -> Result<Option<String>, Error> {
            let Some(claimed) = keys.get(canonical) else { return Ok(None) };
            let values: Vec<&(&str, &str)> = claimed.names.iter().filter(|(_, v)| !v.is_empty()).collect();
            if let Some(first) = values.first() {
                if values.iter().any(|(_, v)| *v != first.1) {
                    let names = values.iter().map(|(k, _)| *k).collect::<Vec<_>>().join(", ");
                    return Err(Error::ConflictingKeys { alias: alias.clone(), key: canonical, names });
                }
                return Ok(Some(first.1.to_string()));
            }
            Ok(None)
        };
        let invalid = |key: &'static str, reason: String| Error::InvalidValue { alias: alias.clone(), key, reason };
        let secret = |canonical: &'static str| get(canonical).map(|v| v.map(SecretString::from));

        let host = get("HOST")?;
        let vault = keys
            .get(VAULT_KEY)
            .and_then(|c| c.names.first())
            .map(|(_, v)| v.to_string())
            .filter(|v| !v.is_empty());
        if host.is_none() && vault.is_none() {
            incomplete.push(alias.clone());
            continue;
        }
        let port = match get("PORT")? {
            Some(p) => Some(p.parse::<u16>().map_err(|_| invalid("PORT", "not a port number".into()))?),
            None => None,
        };
        let escalate = match get("ESCALATE")?.as_deref().map(str::to_ascii_lowercase).as_deref() {
            None => None,
            Some("su") => Some(Escalate::Su),
            Some("sudo") => Some(Escalate::Sudo),
            Some("none") => Some(Escalate::None),
            Some(_) => return Err(invalid("ESCALATE", "expected su, sudo or none".into())),
        };
        let auth = match get("AUTH")? {
            Some(list) => Some(parse_auth(&list).map_err(|reason| invalid("AUTH", reason))?),
            None => None,
        };
        servers.insert(
            alias.clone(),
            Server {
                alias: alias.clone(),
                host,
                port,
                user: get("USER")?,
                pass: secret("PASS")?,
                key: get("KEY")?,
                key_pass: secret("KEY_PASS")?,
                root_user: get("ROOT_USER")?,
                root_pass: secret("ROOT_PASS")?,
                sudo_pass: secret("SUDO_PASS")?,
                escalate,
                auth,
                vault,
            },
        );
    }
    Ok(Config { servers, incomplete, default_alias, sources, warnings, settings: Settings::default() })
}

fn parse_auth(list: &str) -> Result<Vec<AuthMethod>, String> {
    let mut out = Vec::new();
    for item in list.split(',').map(|s| s.trim().to_ascii_lowercase()).filter(|s| !s.is_empty()) {
        out.push(match item.as_str() {
            "key" => AuthMethod::Key,
            "password" => AuthMethod::Password,
            "kbdint" => AuthMethod::Kbdint,
            "agent" => AuthMethod::Agent,
            other => return Err(format!("unknown method '{other}', expected key, password, kbdint or agent")),
        });
    }
    if out.is_empty() { Err("empty list".into()) } else { Ok(out) }
}

pub fn infer_auth(has_key: bool, has_password: bool) -> Vec<AuthMethod> {
    let mut methods = Vec::new();
    if has_key {
        methods.push(AuthMethod::Key);
    }
    if has_password {
        methods.extend([AuthMethod::Password, AuthMethod::Kbdint]);
    }
    methods
}

fn classify(key: &str) -> Option<(String, &'static str, &'static str)> {
    let mut best: Option<(String, &'static str, &'static str)> = None;
    let mut consider = |alias: String, canonical: &'static str, name: &'static str| {
        if best.as_ref().is_none_or(|(_, _, n)| name.len() > n.len()) {
            best = Some((alias, canonical, name));
        }
    };
    let vault_names: [(&'static str, &'static str); 1] = [(VAULT_KEY, VAULT_KEY)];
    let specs = KEYS
        .iter()
        .flat_map(|s| s.names.iter().map(move |n| (s.canonical, *n)))
        .chain(vault_names);
    for (canonical, name) in specs {
        if key == name {
            consider(DEFAULT_ALIAS.to_string(), canonical, name);
        } else if let Some(prefix) = key.strip_suffix(name).and_then(|p| p.strip_suffix('_'))
            && !prefix.is_empty()
        {
            consider(normalize_alias(prefix), canonical, name);
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use secrecy::ExposeSecret;

    fn cfg(text: &str) -> Config {
        let map = dotenv::parse(text, Path::new("t.env")).unwrap();
        build(&map, vec![]).unwrap()
    }

    #[test]
    fn multi_server_prefix_blocks() {
        let c = cfg("ZEUS_HOST=10.0.0.1\nZEUS_USER=deploy\nZEUS_PASS=p\nPROD_WEB_HOST=10.0.0.2\nPROD_WEB_PORT=2200\nPROD_WEB_KEY=~/.ssh/id\nDEFAULT=zeus\n");
        assert_eq!(c.aliases(), ["prod_web", "zeus"]);
        let z = c.select(None).unwrap();
        assert_eq!((z.alias.as_str(), z.user(), z.port()), ("zeus", "deploy", 22));
        assert!(z.auth.is_none());
        let p = c.select(Some("PROD-WEB")).unwrap();
        assert_eq!((p.port, p.user()), (Some(2200), "root"));
    }

    #[test]
    fn auth_is_inferred_from_what_is_present() {
        assert_eq!(infer_auth(true, false), [AuthMethod::Key]);
        assert_eq!(infer_auth(false, true), [AuthMethod::Password, AuthMethod::Kbdint]);
        assert_eq!(infer_auth(true, true), [AuthMethod::Key, AuthMethod::Password, AuthMethod::Kbdint]);
        assert!(infer_auth(false, false).is_empty());
    }

    #[test]
    fn a_vault_reference_stands_in_for_a_missing_host() {
        let c = cfg("ZEUS_VAULT=bw://zeus\nLONELY_PASS=x\n");
        let z = c.select(Some("zeus")).unwrap();
        assert!(z.host.is_none());
        assert_eq!(z.vault.as_deref(), Some("bw://zeus"));
        assert_eq!(c.incomplete, ["lonely"]);
    }

    #[test]
    fn longest_suffix_wins() {
        let c = cfg("ZEUS_HOST=h\nZEUS_ROOT_PASS=r\nZEUS_KEY_PASS=k\nZEUS_PASS=p\nZEUS_ROOT_USER=adm\n");
        let z = c.select(Some("zeus")).unwrap();
        assert_eq!(z.root_pass.as_ref().unwrap().expose_secret(), "r");
        assert_eq!(z.key_pass.as_ref().unwrap().expose_secret(), "k");
        assert_eq!(z.pass.as_ref().unwrap().expose_secret(), "p");
        assert_eq!(z.root_user(), "adm");
        assert_eq!(c.aliases(), ["zeus"]);
    }

    #[test]
    fn bare_keys_form_default_and_legacy_keys_warn() {
        let c = cfg("SSH_HOST=h\nSSH_USER=u\nPASSWORD=p\nCERT=/k\nIP_NOT_A_KEY=1\n");
        let s = c.select(None).unwrap();
        assert_eq!((s.alias.as_str(), s.host.as_deref(), s.user()), ("default", Some("h"), "u"));
        let warned: Vec<&str> = c.warnings.iter().map(|w| w.key.as_str()).collect();
        assert_eq!(warned, ["CERT", "PASSWORD", "SSH_HOST", "SSH_USER"]);
        assert!(c.warnings.iter().any(|w| w.key == "SSH_HOST" && w.use_instead == "HOST"));
    }

    #[test]
    fn legacy_prefixed_key_names_its_replacement() {
        let c = cfg("ZEUS_IP=1.1.1.1\n");
        assert_eq!(c.warnings[0].key, "ZEUS_IP");
        assert_eq!(c.warnings[0].use_instead, "ZEUS_HOST");
    }

    #[test]
    fn conflicting_forms_are_an_error_equal_forms_are_not() {
        let map = dotenv::parse("ZEUS_HOST=a\nZEUS_IP=b\n", Path::new("t.env")).unwrap();
        assert_eq!(build(&map, vec![]).unwrap_err().code(), "conflicting_keys");
        let map = dotenv::parse("ZEUS_HOST=a\nZEUS_IP=a\n", Path::new("t.env")).unwrap();
        assert!(build(&map, vec![]).is_ok());
    }

    #[test]
    fn selection_errors_are_explicit() {
        let c = cfg("A_HOST=1\nB_HOST=2\nC_PASS=x\n");
        assert_eq!(c.select(None).unwrap_err().code(), "no_default_server");
        assert_eq!(c.select(Some("nope")).unwrap_err().code(), "unknown_server");
        assert_eq!(c.select(Some("c")).unwrap_err().code(), "missing_host");
    }

    #[test]
    fn invalid_values_are_rejected() {
        for bad in ["Z_HOST=h\nZ_PORT=99999\n", "Z_HOST=h\nZ_ESCALATE=doas\n", "Z_HOST=h\nZ_AUTH=magic\n"] {
            let map = dotenv::parse(bad, Path::new("t.env")).unwrap();
            assert_eq!(build(&map, vec![]).unwrap_err().code(), "invalid_value");
        }
    }

    #[test]
    fn empty_values_count_as_unset() {
        let c = cfg("Z_HOST=h\nZ_PASS=\nZ_KEY=\n");
        let z = c.select(None).unwrap();
        assert!(z.pass.is_none() && z.key.is_none() && z.auth.is_none());
    }

    #[test]
    fn layering_and_process_env_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let proj = dir.path().join("proj");
        let sub = proj.join("sub");
        std::fs::create_dir_all(home.join(".config").join("sshhh")).unwrap();
        std::fs::create_dir_all(proj.join(".local")).unwrap();
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(home.join(".config/sshhh/servers.env"), "ZEUS_HOST=global\nZEUS_PASS=g\n").unwrap();
        std::fs::write(proj.join(".env"), "ZEUS_HOST=plain\nZEUS_USER=plain\n").unwrap();
        std::fs::write(proj.join(".local/.env"), "ZEUS_USER=local\n").unwrap();
        let env = BTreeMap::from([
            ("USER".to_string(), "shelluser".to_string()),
            ("SSHHH_ZEUS_PORT".to_string(), "2222".to_string()),
            ("SSH_SERVER".to_string(), "zeus".to_string()),
        ]);
        let opts = LoadOptions { env_file: None, cwd: sub, home: Some(home), process_env: env };
        let c = Config::load(&opts).unwrap();
        let z = c.select(None).unwrap();
        assert_eq!((z.host.as_deref(), z.user(), z.port()), (Some("plain"), "local", 2222));
        assert_eq!(z.pass.as_ref().unwrap().expose_secret(), "g");
        assert_eq!(c.sources.len(), 3);
    }

    #[test]
    fn launch_capable_settings_are_ignored_in_project_files() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let proj = dir.path().join("proj");
        std::fs::create_dir_all(home.join(".config").join("sshhh")).unwrap();
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(home.join(".config/sshhh/servers.env"), "BW_SERVER=https://vault.example.test\nBW_BIN=/opt/bw\n").unwrap();
        std::fs::write(proj.join(".env"), "ZEUS_HOST=h\nBW_BIN=/tmp/evil\nBW_APPDATA=/tmp/evil\n").unwrap();
        let env = BTreeMap::from([("SSHHH_BW_APPDATA".to_string(), "/data/bw".to_string())]);
        let opts = LoadOptions { env_file: None, cwd: proj, home: Some(home), process_env: env };
        let c = Config::load(&opts).unwrap();
        assert_eq!(c.settings.bw_bin.as_deref(), Some("/opt/bw"));
        assert_eq!(c.settings.bw_server.as_deref(), Some("https://vault.example.test"));
        assert_eq!(c.settings.bw_appdata, Some(PathBuf::from("/data/bw")));
        let ignored: Vec<&str> = c.warnings.iter().filter(|w| w.warning == "ignored_in_project_file").map(|w| w.key.as_str()).collect();
        assert_eq!(ignored, ["BW_APPDATA", "BW_BIN"]);
    }

    #[test]
    fn explicit_env_file_is_exclusive() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".env"), "OTHER_HOST=x\n").unwrap();
        let f = dir.path().join("custom.env");
        std::fs::write(&f, "ZEUS_HOST=h\n").unwrap();
        let opts = LoadOptions { env_file: Some(f), cwd: dir.path().into(), home: None, process_env: BTreeMap::new() };
        assert_eq!(Config::load(&opts).unwrap().aliases(), ["zeus"]);
        let missing = LoadOptions { env_file: Some(dir.path().join("nope.env")), ..opts };
        assert_eq!(Config::load(&missing).unwrap_err().code(), "env_unreadable");
    }

    #[test]
    fn no_config_lists_where_it_looked() {
        let dir = tempfile::tempdir().unwrap();
        let opts = LoadOptions { env_file: None, cwd: dir.path().into(), home: None, process_env: BTreeMap::new() };
        let e = Config::load(&opts).unwrap_err();
        assert_eq!(e.code(), "no_config");
        assert!(e.to_string().contains(".env"));
    }
}
