//! Test double for the Bitwarden CLI: copied next to a `data/` directory by the integration tests.
//! Messages and JSON shapes follow the real `bw` (checked against its source, cli-v2026.9.1).

use std::io::Write;
use std::path::PathBuf;

fn main() {
    let exe = std::env::current_exe().expect("exe path");
    let data: PathBuf = exe.parent().expect("exe dir").join("data");
    let args: Vec<String> = std::env::args().skip(1).collect();
    let var = |name: &str| std::env::var(name).unwrap_or_default();

    let mut log = std::fs::OpenOptions::new().create(true).append(true).open(data.join("calls.log")).expect("log");
    writeln!(log, "{} | noint={} session={} appdata={}", args.join(" "), var("BW_NOINTERACTION"), var("BW_SESSION"), var("BITWARDENCLI_APPDATA_DIR")).unwrap();

    let fail = |message: &str| -> ! {
        eprintln!("{message}");
        std::process::exit(1);
    };
    if var("BW_NOINTERACTION") != "true" {
        fail("stub: BW_NOINTERACTION was not set");
    }

    let words: Vec<&str> = args.iter().map(String::as_str).filter(|a| !a.starts_with("--")).collect();
    if words == ["status"] {
        let status = std::fs::read_to_string(data.join("status.json"))
            .unwrap_or_else(|_| r#"{"serverUrl":null,"lastSync":null,"status":"unlocked"}"#.to_string());
        print!("{status}");
        return;
    }
    if data.join("unauthenticated").exists() {
        fail("You are not logged in.");
    }
    if data.join("locked").exists() && var("BW_SESSION") != "good-session" {
        fail("Vault is locked.");
    }
    match words.as_slice() {
        ["get", "item", name] => {
            if data.join(format!("ambiguous-{name}")).exists() {
                fail("More than one result was found. Try getting a specific object by `id` instead. The following objects were found:");
            }
            match std::fs::read_to_string(data.join(format!("item-{name}.json"))) {
                Ok(json) => print!("{json}"),
                Err(_) => fail("Not found."),
            }
        }
        ["get", "totp", name] => match std::fs::read_to_string(data.join(format!("totp-{name}.txt"))) {
            Ok(code) => print!("{code}"),
            Err(_) => fail("Not found."),
        },
        other => fail(&format!("stub: unsupported command {other:?}")),
    }
}
