use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use secrecy::ExposeSecret;
use serde_json::{Value, json};
use sshhh::config::{AuthMethod, Config, LoadOptions, Settings};
use sshhh::resolve::Resolver;
use sshhh::secrets::{Secrets, Shape, parse_reference};

struct Bw {
    dir: tempfile::TempDir,
}

impl Bw {
    fn new() -> Bw {
        let dir = tempfile::tempdir().unwrap();
        let exe = std::env::current_exe().unwrap();
        let stub = exe.parent().unwrap().parent().unwrap().join("examples").join(format!("bw_stub{}", std::env::consts::EXE_SUFFIX));
        assert!(stub.is_file(), "build the example first: {}", stub.display());
        std::fs::copy(&stub, dir.path().join(format!("bw{}", std::env::consts::EXE_SUFFIX))).unwrap();
        std::fs::create_dir(dir.path().join("data")).unwrap();
        Bw { dir }
    }

    fn data(&self, name: &str) -> PathBuf {
        self.dir.path().join("data").join(name)
    }

    fn put(&self, name: &str, content: &str) {
        std::fs::write(self.data(name), content).unwrap();
    }

    fn item(&self, name: &str, value: Value) {
        self.put(&format!("item-{name}.json"), &value.to_string());
    }

    fn settings(&self) -> Settings {
        Settings { bw_bin: Some(self.dir.path().join(format!("bw{}", std::env::consts::EXE_SUFFIX)).display().to_string()), ..Settings::default() }
    }

    fn secrets(&self, env: &[(&str, &str)]) -> Secrets {
        Secrets::new(&self.settings(), env.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(), None)
    }

    fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(self.data("calls.log")).map(|t| t.lines().map(str::to_string).collect()).unwrap_or_default()
    }
}

fn zeus() -> Value {
    json!({
        "id": "0b6f", "name": "zeus", "type": 1,
        "notes": "note body",
        "login": { "username": "deploy", "password": "pw-from-vault", "uris": [{ "uri": "ssh://zeus.example.test" }] },
        "fields": [
            { "name": "API Key", "value": "k-123", "type": 1 },
            { "name": "password", "value": "custom-password", "type": 0 },
            { "name": "HOST", "value": "10.1.2.3", "type": 0 },
            { "name": "PORT", "value": "2222", "type": 0 }
        ]
    })
}

async fn get(secrets: &Secrets, reference: &str) -> Result<String, sshhh::error::Error> {
    let reference = parse_reference(reference).unwrap().unwrap();
    secrets.fetch(&reference, Shape::Line).await.map(|s| s.expose_secret().to_string())
}

#[tokio::test]
async fn standard_fields_come_from_the_login_item() {
    let bw = Bw::new();
    bw.item("zeus", zeus());
    let s = bw.secrets(&[]);
    assert_eq!(get(&s, "bw://zeus").await.unwrap(), "pw-from-vault");
    assert_eq!(get(&s, "bw://zeus/password").await.unwrap(), "pw-from-vault");
    assert_eq!(get(&s, "bw://zeus/Username").await.unwrap(), "deploy");
    assert_eq!(get(&s, "bw://zeus/notes").await.unwrap(), "note body");
    assert_eq!(get(&s, "bw://zeus/uri").await.unwrap(), "ssh://zeus.example.test");
}

#[tokio::test]
async fn custom_fields_by_name_or_forced_with_the_field_prefix() {
    let bw = Bw::new();
    bw.item("zeus", zeus());
    let s = bw.secrets(&[]);
    assert_eq!(get(&s, "bw://zeus/API%20Key").await.unwrap(), "k-123");
    assert_eq!(get(&s, "bw://zeus/field:password").await.unwrap(), "custom-password");
    let err = get(&s, "bw://zeus/field:missing").await.unwrap_err();
    assert!(err.to_string().contains("not_found"), "{err}");
}

#[tokio::test]
async fn an_item_is_fetched_once_however_many_fields_are_read() {
    let bw = Bw::new();
    bw.item("zeus", zeus());
    let s = bw.secrets(&[]);
    for r in ["bw://zeus", "bw://zeus/username", "bw://zeus/notes", "bw://zeus/field:HOST"] {
        get(&s, r).await.unwrap();
    }
    assert_eq!(bw.calls().iter().filter(|c| c.starts_with("get item zeus")).count(), 1);
}

#[tokio::test]
async fn bw_is_driven_non_interactively_and_totp_uses_its_own_command() {
    let bw = Bw::new();
    bw.item("zeus", zeus());
    bw.put("totp-zeus.txt", "123456\n");
    let s = bw.secrets(&[]);
    assert_eq!(get(&s, "bw://zeus/totp").await.unwrap(), "123456");
    let calls = bw.calls();
    assert!(calls[0].starts_with("get totp zeus"), "{calls:?}");
    assert!(calls[0].contains("noint=true"), "{calls:?}");
}

#[tokio::test]
async fn failures_are_classified_and_never_leak_or_fall_back() {
    let bw = Bw::new();
    bw.item("zeus", zeus());
    bw.put("ambiguous-dup", "");
    let s = bw.secrets(&[]);

    let err = get(&s, "bw://nothing").await.unwrap_err();
    assert_eq!(err.code(), "secret_unavailable");
    assert!(err.to_string().contains("not_found"), "{err}");

    let err = get(&s, "bw://dup").await.unwrap_err();
    assert!(err.to_string().contains("ambiguous"), "{err}");

    bw.put("locked", "");
    let s = bw.secrets(&[]);
    let err = get(&s, "bw://zeus").await.unwrap_err();
    assert!(err.to_string().contains("vault_locked") && err.to_string().contains("unlock"), "{err}");
    assert!(!err.to_string().contains("pw-from-vault"));

    bw.put("unauthenticated", "");
    let err = get(&bw.secrets(&[]), "bw://zeus").await.unwrap_err();
    assert!(err.to_string().contains("not_logged_in"), "{err}");
}

#[tokio::test]
async fn a_missing_bw_binary_is_a_clear_error() {
    let settings = Settings { bw_bin: Some("definitely-not-installed-bw".into()), ..Settings::default() };
    let s = Secrets::new(&settings, BTreeMap::new(), None);
    let err = get(&s, "bw://zeus").await.unwrap_err();
    assert!(err.to_string().contains("bw_not_installed"), "{err}");
}

#[tokio::test]
async fn session_and_appdata_reach_bw() {
    let bw = Bw::new();
    bw.item("zeus", zeus());
    bw.put("locked", "");
    let appdata = bw.dir.path().join("own-appdata");
    let mut settings = bw.settings();
    settings.bw_appdata = Some(appdata.clone());
    let s = Secrets::new(&settings, BTreeMap::from([("BW_SESSION".to_string(), "good-session".to_string())]), None);
    assert_eq!(get(&s, "bw://zeus").await.unwrap(), "pw-from-vault");
    let call = &bw.calls()[0];
    assert!(call.contains("session=good-session") && call.contains(&format!("appdata={}", appdata.display())), "{call}");
}

#[tokio::test]
async fn the_expected_server_is_checked_before_any_item_is_read() {
    let bw = Bw::new();
    bw.item("zeus", zeus());
    let mut settings = bw.settings();

    settings.bw_server = Some("https://vault.example.test".into());
    let err = get(&Secrets::new(&settings, BTreeMap::new(), None), "bw://zeus").await.unwrap_err();
    assert!(err.to_string().contains("bw_wrong_server"), "{err}");
    assert!(bw.calls().iter().all(|c| !c.starts_with("get ")), "{:?}", bw.calls());

    bw.put("status.json", r#"{"serverUrl":"https://vault.example.test/","lastSync":null,"status":"unlocked"}"#);
    assert_eq!(get(&Secrets::new(&settings, BTreeMap::new(), None), "bw://zeus").await.unwrap(), "pw-from-vault");

    settings.bw_server = Some("https://bitwarden.com".into());
    bw.put("status.json", r#"{"serverUrl":null,"lastSync":null,"status":"unlocked"}"#);
    assert_eq!(get(&Secrets::new(&settings, BTreeMap::new(), None), "bw://zeus").await.unwrap(), "pw-from-vault");
}

#[tokio::test]
async fn env_and_file_providers() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("pass.txt"), "first line\r\nsecond\n").unwrap();
    std::fs::write(dir.path().join("empty.txt"), "\n").unwrap();
    let s = Secrets::new(&Settings::default(), BTreeMap::from([("DEPLOY_PASS".to_string(), "from env".to_string()), ("BLANK".to_string(), String::new())]), Some(dir.path().into()));

    assert_eq!(get(&s, "env://DEPLOY_PASS").await.unwrap(), "from env");
    for gone in ["env://BLANK", "env://UNSET"] {
        assert!(get(&s, gone).await.unwrap_err().to_string().contains("not_set"));
    }

    let path = dir.path().join("pass.txt");
    assert_eq!(get(&s, &format!("file://{}", path.display())).await.unwrap(), "first line");
    assert_eq!(get(&s, "file://~/pass.txt").await.unwrap(), "first line");
    let whole = parse_reference("file://~/pass.txt").unwrap().unwrap();
    assert_eq!(s.fetch(&whole, Shape::Whole).await.unwrap().expose_secret(), "first line\r\nsecond\n");
    assert!(get(&s, "file://~/empty.txt").await.unwrap_err().to_string().contains("empty"));
    assert!(get(&s, "file://~/missing.txt").await.unwrap_err().to_string().contains("unreadable"));
}

fn config(dir: &Path, text: &str) -> Config {
    std::fs::write(dir.join("servers.env"), text).unwrap();
    Config::load(&LoadOptions { env_file: Some(dir.join("servers.env")), cwd: dir.into(), home: None, process_env: BTreeMap::new() }).unwrap()
}

fn resolver(bw: &Bw, home: &Path) -> Resolver {
    Resolver::new(&bw.settings(), BTreeMap::new(), Some(home.into()), home.into())
}

#[tokio::test]
async fn the_vault_convention_fills_what_the_env_file_leaves_out() {
    let bw = Bw::new();
    bw.item("zeus", zeus());
    let work = tempfile::tempdir().unwrap();
    let cfg = config(work.path(), "ZEUS_VAULT=bw://zeus\nZEUS_USER=ops\n");
    let creds = resolver(&bw, work.path()).credentials(cfg.select(Some("zeus")).unwrap()).await.unwrap();
    assert_eq!((creds.host.as_str(), creds.port, creds.user.as_str()), ("10.1.2.3", 2222, "ops"));
    assert!(creds.pass.is_none() && creds.auth.is_empty(), "absent fields stay unset");
}

#[tokio::test]
async fn a_vault_failure_fails_the_operation_even_when_the_env_has_values() {
    let bw = Bw::new();
    bw.put("locked", "");
    let work = tempfile::tempdir().unwrap();
    let cfg = config(work.path(), "ZEUS_VAULT=bw://zeus\nZEUS_HOST=1.2.3.4\nZEUS_PASS=literal\n");
    let err = resolver(&bw, work.path()).credentials(cfg.select(Some("zeus")).unwrap()).await.err().unwrap();
    assert_eq!(err.code(), "secret_unavailable");
}

#[tokio::test]
async fn policy_fields_on_the_vault_item_restrict_and_a_failed_fetch_is_an_error() {
    let bw = Bw::new();
    bw.item(
        "guarded",
        json!({ "login": {}, "fields": [
            { "name": "READONLY", "value": "true", "type": 0 },
            { "name": "DENY_PATHS", "value": "/etc/**;**/.env", "type": 0 },
        ] }),
    );
    bw.item("plain", json!({ "login": {}, "fields": [] }));
    let work = tempfile::tempdir().unwrap();
    let cfg = config(work.path(), "G_VAULT=bw://guarded\nP_VAULT=bw://plain\nN_HOST=h\n");
    let r = resolver(&bw, work.path());
    let guarded = r.vault_policy(cfg.select(Some("g")).unwrap()).await.unwrap();
    assert!(guarded.readonly);
    assert_eq!(guarded.deny_paths, ["/etc/**", "**/.env"]);
    assert!(!r.vault_policy(cfg.select(Some("p")).unwrap()).await.unwrap().is_restricted());
    assert!(!r.vault_policy(cfg.select(Some("n")).unwrap()).await.unwrap().is_restricted());

    let broken = Bw::new();
    broken.put("locked", "");
    let err = resolver(&broken, work.path()).vault_policy(cfg.select(Some("g")).unwrap()).await.unwrap_err();
    assert_eq!(err.code(), "secret_unavailable");
}

#[tokio::test]
async fn a_vault_item_without_a_host_is_missing_host() {
    let bw = Bw::new();
    bw.item("bare", json!({ "login": { "password": "p" }, "fields": [] }));
    let work = tempfile::tempdir().unwrap();
    let cfg = config(work.path(), "BARE_VAULT=bw://bare\n");
    let err = resolver(&bw, work.path()).credentials(cfg.select(Some("bare")).unwrap()).await.err().unwrap();
    assert_eq!(err.code(), "missing_host");
}

#[tokio::test]
async fn references_in_credential_values_are_resolved() {
    let bw = Bw::new();
    bw.item("zeus", zeus());
    let work = tempfile::tempdir().unwrap();
    let cfg = config(work.path(), "ZEUS_HOST=h\nZEUS_USER=bw://zeus/username\nZEUS_PASS=bw://zeus\n");
    let creds = resolver(&bw, work.path()).credentials(cfg.select(Some("zeus")).unwrap()).await.unwrap();
    assert_eq!(creds.user, "deploy");
    assert_eq!(creds.pass.unwrap().expose_secret(), "pw-from-vault");
    assert_eq!(creds.auth, [AuthMethod::Password, AuthMethod::Kbdint]);
}

#[tokio::test]
async fn keys_are_a_path_a_file_reference_or_vault_material() {
    let bw = Bw::new();
    bw.item("keys", json!({ "notes": "-----BEGIN FAKE-----\nbody\n-----END FAKE-----\n", "login": {}, "fields": [] }));
    let work = tempfile::tempdir().unwrap();
    std::fs::write(work.path().join("id_test"), "key-from-path").unwrap();
    std::fs::write(work.path().join("id_test-cert.pub"), "cert-line").unwrap();
    std::fs::write(work.path().join("other"), "key-from-file-ref").unwrap();
    let cfg = config(
        work.path(),
        &format!(
            "A_HOST=h\nA_KEY=~/id_test\nB_HOST=h\nB_KEY=file://{}\nC_HOST=h\nC_KEY=bw://keys/notes\nD_HOST=h\nD_KEY=nowhere/missing\n",
            work.path().join("other").display()
        ),
    );
    let r = resolver(&bw, work.path());
    let a = r.credentials(cfg.select(Some("a")).unwrap()).await.unwrap();
    assert_eq!((a.key.unwrap().expose_secret(), a.cert.as_deref(), a.auth), ("key-from-path", Some("cert-line"), vec![AuthMethod::Key]));
    let b = r.credentials(cfg.select(Some("b")).unwrap()).await.unwrap();
    assert_eq!((b.key.unwrap().expose_secret(), b.cert), ("key-from-file-ref", None));
    let c = r.credentials(cfg.select(Some("c")).unwrap()).await.unwrap();
    assert!(c.key.unwrap().expose_secret().starts_with("-----BEGIN FAKE-----"));
    let d = r.credentials(cfg.select(Some("d")).unwrap()).await.err().unwrap();
    assert_eq!(d.code(), "key_unusable");
}
