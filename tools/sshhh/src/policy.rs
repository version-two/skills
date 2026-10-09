use serde::{Deserialize, Serialize};

use crate::error::Error;

pub const KEYS: [&str; 5] = ["READONLY", "ALLOW_COMMANDS", "DENY_COMMANDS", "ALLOW_PATHS", "DENY_PATHS"];

const MAX_DEPTH: u8 = 4;
const TRUSTED_BINS: [&str; 6] = ["/bin", "/usr/bin", "/sbin", "/usr/sbin", "/usr/local/bin", "/usr/local/sbin"];
const DANGEROUS_ASSIGNMENTS: [&str; 7] = ["PATH", "LD_PRELOAD", "LD_LIBRARY_PATH", "IFS", "BASH_ENV", "ENV", "SHELLOPTS"];
const SKIPPED_KEYWORDS: [&str; 11] = ["do", "then", "else", "elif", "if", "while", "until", "time", "!", "done", "fi"];
const REJECTED_KEYWORDS: [&str; 6] = ["for", "case", "select", "function", "coproc", "[["];
const SHELLS: [&str; 8] = ["sh", "bash", "dash", "zsh", "ksh", "ash", "su", "busybox"];

/// Wrapper programs and the options of each that consume the next word.
const WRAPPERS: [(&str, &str); 14] = [
    ("env", "uCS"),
    ("sudo", "ughpCDRTU"),
    ("doas", "uC"),
    ("nohup", ""),
    ("nice", "n"),
    ("ionice", "cnp"),
    ("time", ""),
    ("command", ""),
    ("exec", "a"),
    ("timeout", "sk"),
    ("xargs", "nIPLdEsa"),
    ("stdbuf", "ioe"),
    ("setsid", ""),
    ("flock", "wEn"),
];

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Policy {
    pub readonly: bool,
    /// One list per configuration layer; a command must satisfy every layer.
    pub allow_commands: Vec<Vec<String>>,
    pub deny_commands: Vec<String>,
    pub allow_paths: Vec<Vec<String>>,
    pub deny_paths: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    Read,
    Write,
}

fn denied(rule: &'static str, detail: impl Into<String>) -> Error {
    Error::PolicyDenied { rule, detail: detail.into() }
}

fn split_list(value: &str) -> Vec<String> {
    value.split(';').map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned).collect()
}

fn parse_bool(value: &str) -> Result<bool, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Ok(true),
        "false" | "0" | "no" | "off" => Ok(false),
        _ => Err("expected true or false".into()),
    }
}

fn validate_path_pattern(pattern: &str) -> Result<(), String> {
    if pattern.starts_with('~') {
        return Err(format!("'{pattern}': write absolute paths, the remote home directory is not known here"));
    }
    if !pattern.starts_with('/') && !pattern.starts_with('*') {
        return Err(format!("'{pattern}': a path pattern starts with / or **"));
    }
    Ok(())
}

impl Policy {
    pub fn is_restricted(&self) -> bool {
        *self != Policy::default()
    }

    /// Adds one configuration layer. Empty values are unset; `none` in an allow list allows nothing.
    pub fn add_layer(&mut self, key: &str, value: &str) -> Result<(), String> {
        if value.trim().is_empty() {
            return Ok(());
        }
        let list = |value: &str| if value.trim().eq_ignore_ascii_case("none") { Vec::new() } else { split_list(value) };
        match key {
            "READONLY" => self.readonly |= parse_bool(value)?,
            "ALLOW_COMMANDS" => self.allow_commands.push(list(value)),
            "DENY_COMMANDS" => self.deny_commands.extend(split_list(value)),
            "ALLOW_PATHS" => {
                let entries = list(value);
                entries.iter().try_for_each(|p| validate_path_pattern(p))?;
                self.allow_paths.push(entries);
            }
            "DENY_PATHS" => {
                let entries = split_list(value);
                entries.iter().try_for_each(|p| validate_path_pattern(p))?;
                self.deny_paths.extend(entries);
            }
            other => return Err(format!("{other} is not a policy key")),
        }
        Ok(())
    }

    /// A read-only server must not be handed a list that allows every command.
    pub fn validate(&self) -> Result<(), String> {
        let catch_all = self.allow_commands.iter().flatten().find(|p| p.chars().all(|c| c == '*' || c == '?' || c.is_whitespace()));
        match (self.readonly, catch_all) {
            (true, Some(pattern)) => Err(format!("ALLOW_COMMANDS entry '{pattern}' allows every command; list the read-only commands instead")),
            _ => Ok(()),
        }
    }

    pub fn merge(&mut self, other: &Policy) {
        self.readonly |= other.readonly;
        self.allow_commands.extend(other.allow_commands.iter().cloned());
        self.deny_commands.extend(other.deny_commands.iter().cloned());
        self.allow_paths.extend(other.allow_paths.iter().cloned());
        self.deny_paths.extend(other.deny_paths.iter().cloned());
    }

    pub fn path_policy(&self) -> bool {
        !self.allow_paths.is_empty() || !self.deny_paths.is_empty()
    }

    pub fn check_write_operation(&self, what: &str) -> Result<(), Error> {
        if self.readonly { Err(denied("READONLY", format!("{what} would change the server"))) } else { Ok(()) }
    }

    /// `path` must be absolute; callers pass the server's own realpath as well as the given path.
    pub fn check_path(&self, path: &str, access: Access) -> Result<(), Error> {
        if path == "/dev/null" {
            return Ok(());
        }
        if access == Access::Write {
            self.check_write_operation(&format!("writing {path}"))?;
        }
        let Some(segments) = normalize(path) else {
            return Err(denied("ALLOW_PATHS", format!("'{path}' is not an absolute path")));
        };
        if let Some(pattern) = self.deny_paths.iter().find(|p| covers(p, &segments)) {
            return Err(denied("DENY_PATHS", format!("{path} matches {pattern}")));
        }
        for layer in &self.allow_paths {
            if !layer.iter().any(|p| covers(p, &segments)) {
                return Err(denied("ALLOW_PATHS", format!("{path} is not under an allowed path")));
            }
        }
        Ok(())
    }

    pub fn check_exec(&self, command: &str, root: bool) -> Result<(), Error> {
        if !self.is_restricted() {
            return Ok(());
        }
        self.validate().map_err(|reason| denied("READONLY", reason))?;
        if root && self.readonly {
            return Err(denied("READONLY", "privilege escalation is not available on a read-only server"));
        }
        if self.readonly && self.allow_commands.is_empty() {
            return Err(denied(
                "READONLY",
                "a command cannot be shown to be read-only; use get, ls and cat, or list read-only commands in ALLOW_COMMANDS",
            ));
        }
        self.check_script(command, 0)
    }

    fn check_script(&self, script: &str, depth: u8) -> Result<(), Error> {
        let simples = parse_shell(script).map_err(|reason| denied("COMMAND_SYNTAX", format!("{reason}; the command cannot be checked against the policy")))?;
        for simple in simples {
            let argv = strip_prefix(simple.argv).map_err(|reason| denied("COMMAND_SYNTAX", reason))?;
            self.check_command(&argv, depth)?;
            for redirect in &simple.redirects {
                self.check_token_path(&redirect.target, if redirect.write { Access::Write } else { Access::Read })?;
            }
        }
        Ok(())
    }

    fn check_command(&self, argv: &[String], depth: u8) -> Result<(), Error> {
        let Some(first) = argv.first() else { return Ok(()) };
        if depth > MAX_DEPTH {
            return Err(denied("COMMAND_SYNTAX", "commands are nested too deeply to be checked"));
        }
        let name = command_name(first)?;
        let args = &argv[1..];

        if let Some(pattern) = self.deny_commands.iter().find(|p| command_matches(p, name, args)) {
            return Err(denied("DENY_COMMANDS", format!("{} matches {pattern}", describe(name, args))));
        }
        for layer in &self.allow_commands {
            if !layer.iter().any(|p| command_matches(p, name, args)) {
                return Err(denied("ALLOW_COMMANDS", format!("{} is not in the list of allowed commands", describe(name, args))));
            }
        }

        let mut script_index = None;
        if let Some((_, with_value)) = WRAPPERS.iter().find(|(wrapper, _)| *wrapper == name) {
            let inner = skip_wrapper_options(name, with_value, args);
            if self.deny_commands.iter().len() > 0 {
                for (k, token) in inner.iter().enumerate() {
                    let candidate = command_name(token)?;
                    if let Some(pattern) = self.deny_commands.iter().find(|p| command_matches(p, candidate, &inner[k + 1..])) {
                        return Err(denied("DENY_COMMANDS", format!("{} matches {pattern}", describe(candidate, &inner[k + 1..]))));
                    }
                }
            }
            self.check_command(inner, depth + 1)?;
        } else if name == "eval" {
            return self.check_script(&args.join(" "), depth + 1);
        } else if SHELLS.contains(&name)
            && let Some(position) = args.iter().position(|a| a.starts_with('-') && !a.starts_with("--") && a.contains('c'))
            && let Some(script) = args.get(position + 1)
        {
            script_index = Some(position + 1);
            self.check_script(script, depth + 1)?;
        }

        if self.path_policy() {
            for (index, token) in args.iter().enumerate() {
                if Some(index) != script_index {
                    self.check_arg_path(token)?;
                }
            }
        }
        Ok(())
    }

    fn check_arg_path(&self, token: &str) -> Result<(), Error> {
        let value = match token.strip_prefix('-') {
            Some(option) => match option.split_once('=') {
                Some((_, value)) => value,
                None => return Ok(()),
            },
            None => token,
        };
        self.check_token_path(value, Access::Read)
    }

    fn check_token_path(&self, token: &str, access: Access) -> Result<(), Error> {
        if token == "/dev/null" {
            return Ok(());
        }
        if access == Access::Write && self.readonly {
            return Err(denied("READONLY", format!("redirecting output to {token} would change the server")));
        }
        if !self.path_policy() {
            return Ok(());
        }
        let path_like = token.contains('/') || token.starts_with('~') || token == "." || token == "..";
        if !path_like {
            return Ok(());
        }
        if !token.starts_with('/') {
            return Err(denied("ALLOW_PATHS", format!("{token}: write absolute paths when a path policy is set")));
        }
        if token.contains(['*', '?', '[']) {
            return Err(denied("ALLOW_PATHS", format!("{token}: a wildcard in a path cannot be checked")));
        }
        self.check_path(token, access)
    }
}

fn describe(name: &str, args: &[String]) -> String {
    if args.is_empty() { name.to_owned() } else { format!("{name} {}", args.join(" ")) }
}

fn command_name(first: &str) -> Result<&str, Error> {
    match first.rfind('/') {
        None => Ok(first),
        Some(at) => {
            let dir = &first[..at];
            if TRUSTED_BINS.contains(&dir) {
                Ok(&first[at + 1..])
            } else {
                Err(denied("ALLOW_COMMANDS", format!("{first}: a command given by path must live in a system bin directory")))
            }
        }
    }
}

/// A pattern without whitespace names the program; one with whitespace is matched against the
/// program and its arguments.
fn command_matches(pattern: &str, name: &str, args: &[String]) -> bool {
    let chars = |s: &str| s.chars().collect::<Vec<_>>();
    if pattern.contains(char::is_whitespace) {
        wild(&chars(pattern), &chars(&describe(name, args)))
    } else {
        wild(&chars(pattern), &chars(name))
    }
}

fn skip_wrapper_options<'a>(wrapper: &str, with_value: &str, args: &'a [String]) -> &'a [String] {
    let mut i = 0;
    while let Some(token) = args.get(i) {
        let is_assignment = wrapper == "env" && is_assignment(token);
        if is_assignment {
            i += 1;
        } else if let Some(option) = token.strip_prefix('-').filter(|o| !o.is_empty() && !o.starts_with('-')) {
            i += if option.len() == 1 && with_value.contains(option) { 2 } else { 1 };
        } else if token.starts_with("--") && token.len() > 2 {
            i += 1;
        } else {
            break;
        }
    }
    let rest = &args[i.min(args.len())..];
    let takes_duration = matches!(wrapper, "timeout" | "flock");
    match rest.first() {
        Some(word) if takes_duration && word.chars().next().is_some_and(|c| c.is_ascii_digit() || c == '/') && wrapper == "timeout" => &rest[1..],
        _ => rest,
    }
}

fn is_assignment(token: &str) -> bool {
    match token.split_once('=') {
        Some((name, _)) => !name.is_empty() && !name.starts_with(|c: char| c.is_ascii_digit()) && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
        None => false,
    }
}

fn strip_prefix(mut argv: Vec<String>) -> Result<Vec<String>, String> {
    loop {
        let Some(first) = argv.first() else { return Ok(argv) };
        if is_assignment(first) {
            let name = first.split('=').next().unwrap_or_default();
            if DANGEROUS_ASSIGNMENTS.contains(&name) || name.starts_with("LD_") {
                return Err(format!("assigning {name} changes how commands resolve"));
            }
        } else if REJECTED_KEYWORDS.contains(&first.as_str()) {
            return Err(format!("'{first}' constructs are not supported"));
        } else if !SKIPPED_KEYWORDS.contains(&first.as_str()) && first != "esac" {
            return Ok(argv);
        }
        argv.remove(0);
    }
}

/// `*` matches any run of characters, `?` one character.
fn wild(pattern: &[char], text: &[char]) -> bool {
    match pattern.split_first() {
        None => text.is_empty(),
        Some(('*', rest)) => (0..=text.len()).any(|i| wild(rest, &text[i..])),
        Some(('?', rest)) => text.split_first().is_some_and(|(_, tail)| wild(rest, tail)),
        Some((c, rest)) => text.split_first().is_some_and(|(t, tail)| t == c && wild(rest, tail)),
    }
}

pub fn normalize(path: &str) -> Option<Vec<String>> {
    if !path.starts_with('/') {
        return None;
    }
    let mut out: Vec<String> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            other => out.push(other.to_owned()),
        }
    }
    Some(out)
}

fn segments_match(pattern: &[&str], path: &[String]) -> bool {
    match pattern.split_first() {
        None => path.is_empty(),
        Some((&"**", rest)) => (0..=path.len()).any(|i| segments_match(rest, &path[i..])),
        Some((segment, rest)) => path.split_first().is_some_and(|(head, tail)| {
            let (p, t): (Vec<char>, Vec<char>) = (segment.chars().collect(), head.chars().collect());
            wild(&p, &t) && segments_match(rest, tail)
        }),
    }
}

/// A pattern covers a path when it matches the path or any directory above it.
fn covers(pattern: &str, path: &[String]) -> bool {
    let segments: Vec<&str> = pattern.split('/').filter(|s| !s.is_empty()).collect();
    (0..=path.len()).rev().any(|end| segments_match(&segments, &path[..end]))
}

#[derive(Debug, Default, PartialEq, Eq)]
struct Simple {
    argv: Vec<String>,
    redirects: Vec<Redirect>,
}

#[derive(Debug, PartialEq, Eq)]
struct Redirect {
    write: bool,
    target: String,
}

struct Parser<'a> {
    chars: std::iter::Peekable<std::str::Chars<'a>>,
    out: Vec<Simple>,
    cur: Simple,
    word: Option<String>,
    redirect: Option<bool>,
}

impl Parser<'_> {
    fn flush(&mut self) {
        if let Some(word) = self.word.take() {
            match self.redirect.take() {
                Some(write) => self.cur.redirects.push(Redirect { write, target: word }),
                None => self.cur.argv.push(word),
            }
        }
    }

    fn end_command(&mut self) -> Result<(), String> {
        self.flush();
        if self.redirect.is_some() {
            return Err("a redirection has no target".into());
        }
        let done = std::mem::take(&mut self.cur);
        if !done.argv.is_empty() || !done.redirects.is_empty() {
            self.out.push(done);
        }
        Ok(())
    }

    fn word(&mut self) -> &mut String {
        self.word.get_or_insert_with(String::new)
    }

    /// A word made only of digits directly before `>` or `<` is a file descriptor.
    fn drop_fd(&mut self) {
        if self.word.as_deref().is_some_and(|w| !w.is_empty() && w.chars().all(|c| c.is_ascii_digit())) {
            self.word = None;
        } else {
            self.flush();
        }
    }
}

fn parse_shell(command: &str) -> Result<Vec<Simple>, String> {
    let mut p = Parser { chars: command.chars().peekable(), out: Vec::new(), cur: Simple::default(), word: None, redirect: None };
    while let Some(c) = p.chars.next() {
        match c {
            ' ' | '\t' | '\r' => p.flush(),
            '\n' | ';' => p.end_command()?,
            '|' => {
                match p.chars.peek() {
                    Some('&') => return Err("'|&' is not supported".into()),
                    Some('|') => {
                        p.chars.next();
                    }
                    _ => {}
                }
                p.end_command()?;
            }
            '&' => {
                if p.chars.next_if_eq(&'&').is_none() {
                    return Err("background execution is not supported".into());
                }
                p.end_command()?;
            }
            '>' => {
                p.drop_fd();
                p.chars.next_if_eq(&'>');
                if p.chars.next_if_eq(&'&').is_some() {
                    if p.chars.next_if(|c| c.is_ascii_digit() || *c == '-').is_none() {
                        return Err("redirecting to a name after '>&' is not supported".into());
                    }
                    while p.chars.next_if(char::is_ascii_digit).is_some() {}
                    continue;
                }
                p.chars.next_if_eq(&'|');
                p.redirect = Some(true);
            }
            '<' => {
                p.drop_fd();
                if matches!(p.chars.peek(), Some('<' | '(' | '&')) {
                    return Err("here-documents and process substitution are not supported".into());
                }
                p.redirect = Some(false);
            }
            '\'' => {
                let mut closed = false;
                p.word();
                for q in p.chars.by_ref() {
                    if q == '\'' {
                        closed = true;
                        break;
                    }
                    p.word.get_or_insert_with(String::new).push(q);
                }
                if !closed {
                    return Err("unterminated quote".into());
                }
            }
            '"' => {
                let mut closed = false;
                p.word();
                while let Some(q) = p.chars.next() {
                    match q {
                        '"' => {
                            closed = true;
                            break;
                        }
                        '$' | '`' => return Err("expansion inside double quotes is not supported".into()),
                        '\\' => match p.chars.next() {
                            Some(e @ ('"' | '\\' | '$' | '`')) => p.word().push(e),
                            Some('\n') => {}
                            Some(other) => {
                                p.word().push('\\');
                                p.word().push(other);
                            }
                            None => return Err("unterminated quote".into()),
                        },
                        other => p.word().push(other),
                    }
                }
                if !closed {
                    return Err("unterminated quote".into());
                }
            }
            '\\' => match p.chars.next() {
                Some('\n') => {}
                Some(escaped) => p.word().push(escaped),
                None => return Err("dangling backslash".into()),
            },
            '$' | '`' => return Err("shell expansion is not supported".into()),
            '(' | ')' | '{' | '}' => return Err("grouping and brace expansion are not supported".into()),
            '#' if p.word.is_none() => {
                while p.chars.next_if(|c| *c != '\n').is_some() {}
            }
            other => p.word().push(other),
        }
    }
    p.end_command()?;
    Ok(p.out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(entries: &[(&str, &str)]) -> Policy {
        let mut p = Policy::default();
        for (key, value) in entries {
            p.add_layer(key, value).unwrap();
        }
        p
    }

    fn rule(result: Result<(), Error>) -> &'static str {
        match result.unwrap_err() {
            Error::PolicyDenied { rule, .. } => rule,
            other => panic!("unexpected {other}"),
        }
    }

    #[test]
    fn no_policy_allows_everything_including_unparsable_commands() {
        assert!(Policy::default().check_exec("rm -rf $HOME; `x`", true).is_ok());
        assert!(Policy::default().check_path("/etc/shadow", Access::Write).is_ok());
    }

    #[test]
    fn values_are_validated_and_empty_means_unset() {
        let mut p = Policy::default();
        assert!(p.add_layer("READONLY", "maybe").is_err());
        assert!(p.add_layer("ALLOW_PATHS", "~/x").is_err());
        assert!(p.add_layer("DENY_PATHS", "relative/dir").is_err());
        p.add_layer("READONLY", "").unwrap();
        p.add_layer("ALLOW_COMMANDS", " ").unwrap();
        assert!(!p.is_restricted());
        p.add_layer("READONLY", "No").unwrap();
        assert!(!p.readonly);
        p.add_layer("READONLY", "yes").unwrap();
        assert!(p.readonly);
    }

    #[test]
    fn readonly_blocks_writes_root_and_unlisted_commands() {
        let ro = policy(&[("READONLY", "true")]);
        assert_eq!(rule(ro.check_exec("ls", false)), "READONLY");
        assert_eq!(rule(ro.check_write_operation("put")), "READONLY");
        assert_eq!(rule(ro.check_path("/tmp/x", Access::Write)), "READONLY");
        assert!(ro.check_path("/tmp/x", Access::Read).is_ok());

        let listed = policy(&[("READONLY", "true"), ("ALLOW_COMMANDS", "ls;cat;df;systemctl status *")]);
        assert!(listed.check_exec("ls -la /var/log | cat", false).is_ok());
        assert!(listed.check_exec("systemctl status nginx", false).is_ok());
        assert_eq!(rule(listed.check_exec("systemctl restart nginx", false)), "ALLOW_COMMANDS");
        assert_eq!(rule(listed.check_exec("ls", true)), "READONLY");
        assert_eq!(rule(listed.check_exec("ls > /tmp/out", false)), "READONLY");
        assert_eq!(rule(listed.check_exec("ls 2>&1 >> /tmp/out", false)), "READONLY");
        assert!(listed.check_exec("ls /x 2>/dev/null", false).is_ok());
    }

    #[test]
    fn a_catch_all_allow_list_does_not_make_a_readonly_server_safe() {
        let p = policy(&[("READONLY", "true"), ("ALLOW_COMMANDS", "ls;*")]);
        assert!(p.validate().is_err());
        assert_eq!(rule(p.check_exec("rm -rf /", false)), "READONLY");
        assert!(policy(&[("ALLOW_COMMANDS", "*")]).validate().is_ok());
    }

    #[test]
    fn readonly_cannot_be_lowered_by_a_later_layer() {
        let mut p = policy(&[("READONLY", "true")]);
        p.add_layer("READONLY", "false").unwrap();
        assert!(p.readonly);
        let mut merged = policy(&[("READONLY", "true")]);
        merged.merge(&Policy::default());
        assert!(merged.readonly);
    }

    #[test]
    fn allow_lists_are_default_deny_and_every_layer_must_agree() {
        let mut p = policy(&[("ALLOW_COMMANDS", "ls;cat;grep")]);
        assert!(p.check_exec("grep -r foo /x", false).is_ok());
        assert_eq!(rule(p.check_exec("rm x", false)), "ALLOW_COMMANDS");
        p.add_layer("ALLOW_COMMANDS", "ls").unwrap();
        assert!(p.check_exec("ls", false).is_ok());
        assert_eq!(rule(p.check_exec("cat x", false)), "ALLOW_COMMANDS");
        assert_eq!(rule(policy(&[("ALLOW_COMMANDS", "none")]).check_exec("ls", false)), "ALLOW_COMMANDS");
    }

    #[test]
    fn every_command_of_a_compound_line_is_checked() {
        let p = policy(&[("ALLOW_COMMANDS", "ls;echo")]);
        for line in ["ls; rm x", "ls && rm x", "ls || rm x", "ls | rm x", "ls\nrm x", "echo a; ls; echo b; rm"] {
            assert_eq!(rule(p.check_exec(line, false)), "ALLOW_COMMANDS", "{line}");
        }
        assert!(p.check_exec("echo 'a; rm x' \"b && rm\"", false).is_ok());
    }

    #[test]
    fn deny_wins_and_sees_through_paths_quotes_wrappers_and_shells() {
        let p = policy(&[("DENY_COMMANDS", "rm;dd;shutdown;systemctl stop *")]);
        for line in [
            "rm -rf x",
            "/bin/rm x",
            "\\rm x",
            "'r'm x",
            "sudo rm x",
            "sudo -u root rm x",
            "env FOO=1 rm x",
            "nohup rm x &&",
            "timeout 5 rm x",
            "xargs -n1 rm",
            "sh -c 'rm x'",
            "bash -lc \"echo hi; rm x\"",
            "su -c 'dd if=/dev/zero of=/x'",
            "sudo sh -c 'shutdown now'",
            "eval rm x",
            "systemctl stop nginx",
            "for f in a; do rm x; done",
        ] {
            let result = p.check_exec(line, false);
            assert!(result.is_err(), "{line} should be denied");
        }
        assert!(p.check_exec("systemctl status nginx", false).is_ok());
        assert!(p.check_exec("ls /bin/rmdir-not", false).is_ok());
    }

    #[test]
    fn a_command_by_untrusted_path_is_denied() {
        let p = policy(&[("ALLOW_COMMANDS", "ls")]);
        assert!(p.check_exec("/usr/bin/ls", false).is_ok());
        assert_eq!(rule(p.check_exec("/tmp/evil/ls", false)), "ALLOW_COMMANDS");
        assert_eq!(rule(p.check_exec("./ls", false)), "ALLOW_COMMANDS");
    }

    #[test]
    fn constructs_that_cannot_be_checked_are_refused_when_a_policy_exists() {
        let p = policy(&[("DENY_COMMANDS", "rm")]);
        for line in ["echo $(rm x)", "echo `rm x`", "echo $HOME", "echo \"$x\"", "(rm x)", "{ rm x; }", "ls &", "cat <<EOF", "cat <(ls)", "ls >&out", "FOO=1 PATH=/tmp ls", "echo 'unterminated"] {
            assert_eq!(rule(p.check_exec(line, false)), "COMMAND_SYNTAX", "{line}");
        }
        assert!(p.check_exec("echo a 2>&1 # rm x", false).is_ok());
    }

    #[test]
    fn path_lists_cover_directories_and_globs_and_deny_wins() {
        let p = policy(&[("ALLOW_PATHS", "/var/www;/etc/nginx/**/*.conf"), ("DENY_PATHS", "**/.env;/var/www/secret;**/*.pem")]);
        assert!(p.check_path("/var/www/site/index.php", Access::Read).is_ok());
        assert!(p.check_path("/var/www", Access::Read).is_ok());
        assert!(p.check_path("/etc/nginx/conf.d/a.conf", Access::Read).is_ok());
        assert_eq!(rule(p.check_path("/var/www2/x", Access::Read)), "ALLOW_PATHS");
        assert_eq!(rule(p.check_path("/etc/nginx/nginx.conf.bak", Access::Read)), "ALLOW_PATHS");
        assert_eq!(rule(p.check_path("/var/www/site/.env", Access::Read)), "DENY_PATHS");
        assert_eq!(rule(p.check_path("/var/www/secret/a", Access::Read)), "DENY_PATHS");
        assert_eq!(rule(p.check_path("/var/www/ssl/key.pem", Access::Read)), "DENY_PATHS");
        assert_eq!(rule(p.check_path("/var/www/../../etc/shadow", Access::Read)), "ALLOW_PATHS");
        assert_eq!(rule(p.check_path("relative", Access::Read)), "ALLOW_PATHS");
    }

    #[test]
    fn path_layers_only_tighten() {
        let mut p = policy(&[("ALLOW_PATHS", "/var")]);
        p.add_layer("ALLOW_PATHS", "/var/log").unwrap();
        assert!(p.check_path("/var/log/syslog", Access::Read).is_ok());
        assert_eq!(rule(p.check_path("/var/www/a", Access::Read)), "ALLOW_PATHS");
        assert_eq!(rule(policy(&[("ALLOW_PATHS", "none")]).check_path("/x", Access::Read)), "ALLOW_PATHS");
    }

    #[test]
    fn commands_are_checked_for_the_paths_they_name() {
        let p = policy(&[("ALLOW_PATHS", "/var/log"), ("DENY_PATHS", "/var/log/auth*")]);
        assert!(p.check_exec("tail -n 5 /var/log/syslog", false).is_ok());
        assert!(p.check_exec("echo hello", false).is_ok());
        assert_eq!(rule(p.check_exec("cat /etc/shadow", false)), "ALLOW_PATHS");
        assert_eq!(rule(p.check_exec("cat /var/log/../../etc/shadow", false)), "ALLOW_PATHS");
        assert_eq!(rule(p.check_exec("cat /var/log/auth.log", false)), "DENY_PATHS");
        assert_eq!(rule(p.check_exec("cat logs/x", false)), "ALLOW_PATHS");
        assert_eq!(rule(p.check_exec("cat ~/x", false)), "ALLOW_PATHS");
        assert_eq!(rule(p.check_exec("cat /var/log/*", false)), "ALLOW_PATHS");
        assert_eq!(rule(p.check_exec("tar --file=/etc/x t", false)), "ALLOW_PATHS");
        assert_eq!(rule(p.check_exec("echo x > /etc/passwd", false)), "ALLOW_PATHS");
        assert_eq!(rule(p.check_exec("sh -c 'cat /etc/shadow'", false)), "ALLOW_PATHS");
        assert!(p.check_exec("sh -c 'cat /var/log/syslog'", false).is_ok());
    }

    #[test]
    fn parser_unquotes_like_a_posix_shell() {
        let parsed = parse_shell("a 'b c' \"d\\\"e\" f\\ g 2>&1 > out 1>>log < in").unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].argv, ["a", "b c", "d\"e", "f g"]);
        let targets: Vec<(bool, &str)> = parsed[0].redirects.iter().map(|r| (r.write, r.target.as_str())).collect();
        assert_eq!(targets, [(true, "out"), (true, "log"), (false, "in")]);
        assert_eq!(parse_shell("echo ''").unwrap()[0].argv, ["echo", ""]);
    }
}
