//! Open-with handlers (~/.config/kallos/handlers) and process spawning.
//!
//! Config format, same as the C app:
//!     ext, ext: command with {FILE}
//! Extension match is case-insensitive and the last matching line wins.
//!
//! Differences from the C wordexp/fork path, on purpose:
//! - The command template is tokenized at parse time and {FILE} substituted
//!   per token afterward, so paths with spaces stay one argv entry (the C
//!   version wordexp'd after substitution and split them).
//! - Expansion is whitespace-splitting plus leading-~ home expansion only:
//!   no globbing, no $VARS, no quoting.
//! - Children are reaped (see `reap`), so no zombies accumulate.

use std::ffi::{OsStr, OsString};
use std::io;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::Path;
use std::process::{Child, Command, Stdio};

pub struct Handler {
    pub exts: Vec<String>,
    pub command: Vec<String>,
}

pub fn read_handlers(path: &Path) -> Vec<Handler> {
    let Ok(content) = std::fs::read_to_string(path) else { return Vec::new() };
    let home = std::env::var_os("HOME");
    let mut handlers = Vec::new();
    for line in content.lines() {
        let Some((exts, command)) = line.split_once(':') else { continue };
        let exts: Vec<String> = exts
            .split(',')
            .map(|e| e.trim().to_ascii_lowercase())
            .filter(|e| !e.is_empty())
            .collect();
        let command: Vec<String> = tokenize(command)
            .into_iter()
            .map(|tok| expand_tilde(&tok, home.as_deref()))
            .collect();
        if exts.is_empty() || command.is_empty() {
            continue;
        }
        handlers.push(Handler { exts, command });
    }
    handlers
}

/// Split a command template into argv tokens, honouring single/double quotes:
/// quotes group whitespace into one token and are stripped from the result, so
/// `imv "{FILE}"` yields `["imv", "{FILE}"]` (the C app relied on wordexp for
/// this). No escapes or nested quoting — the config format is intentionally
/// tiny.
fn tokenize(s: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut cur = String::new();
    let mut has_token = false;
    let mut quote: Option<char> = None;
    for c in s.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => cur.push(c),
            None if c == '"' || c == '\'' => {
                quote = Some(c);
                has_token = true;
            }
            None if c.is_whitespace() => {
                if has_token {
                    tokens.push(std::mem::take(&mut cur));
                    has_token = false;
                }
            }
            None => {
                cur.push(c);
                has_token = true;
            }
        }
    }
    if has_token {
        tokens.push(cur);
    }
    tokens
}

fn expand_tilde(tok: &str, home: Option<&OsStr>) -> String {
    if let Some(home) = home {
        if tok == "~" {
            return home.to_string_lossy().into_owned();
        }
        if let Some(rest) = tok.strip_prefix("~/") {
            return format!("{}/{}", home.to_string_lossy(), rest);
        }
    }
    tok.to_string()
}

/// Last matching handler wins, like the C loop.
pub fn find_handler<'a>(handlers: &'a [Handler], file: &Path) -> Option<&'a Handler> {
    let ext = file.extension()?.to_str()?.to_ascii_lowercase();
    handlers.iter().rev().find(|h| h.exts.iter().any(|e| *e == ext))
}

/// Replace every "{FILE}" occurrence in a token with the raw path bytes.
fn substitute(tok: &str, file: &Path) -> OsString {
    if !tok.contains("{FILE}") {
        return OsString::from(tok);
    }
    let mut out = Vec::new();
    let mut rest = tok;
    while let Some(idx) = rest.find("{FILE}") {
        out.extend_from_slice(rest[..idx].as_bytes());
        out.extend_from_slice(file.as_os_str().as_bytes());
        rest = &rest[idx + "{FILE}".len()..];
    }
    out.extend_from_slice(rest.as_bytes());
    OsString::from_vec(out)
}

pub fn spawn_handler(
    handler: &Handler,
    file: &Path,
    children: &mut Vec<Child>,
) -> io::Result<()> {
    let args: Vec<OsString> = handler.command.iter().map(|t| substitute(t, file)).collect();
    let refs: Vec<&OsStr> = args.iter().map(|a| a.as_os_str()).collect();
    spawn(refs[0], &refs[1..], children)
}

/// Spawn a command with an argument vector (execvp semantics: PATH search
/// unless the program contains '/'). Stdio is detached.
pub fn spawn<S: AsRef<OsStr>>(
    program: S,
    args: &[S],
    children: &mut Vec<Child>,
) -> io::Result<()> {
    let child = Command::new(program.as_ref())
        .args(args.iter().map(|a| a.as_ref()))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    children.push(child);
    Ok(())
}

/// Sweep exited children (the C app never waited -> zombies).
pub fn reap(children: &mut Vec<Child>) {
    children.retain_mut(|c| matches!(c.try_wait(), Ok(None)));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenize_strips_quotes_and_groups_spaces() {
        assert_eq!(tokenize("imv \"{FILE}\""), vec!["imv", "{FILE}"]);
        assert_eq!(tokenize("imv '{FILE}'"), vec!["imv", "{FILE}"]);
        assert_eq!(tokenize("imv {FILE}"), vec!["imv", "{FILE}"]);
        assert_eq!(tokenize("a \"b c\" d"), vec!["a", "b c", "d"]);
        assert_eq!(tokenize("  trailing  "), vec!["trailing"]);
    }

    #[test]
    fn quoted_placeholder_substitutes_without_literal_quotes() {
        // The whole point of the bug fix: `imv "{FILE}"` must pass the bare
        // path, not a path wrapped in quote characters.
        let toks = tokenize("imv \"{FILE}\"");
        let path = Path::new("/home/thomas/my pic.png");
        let args: Vec<OsString> = toks.iter().map(|t| substitute(t, path)).collect();
        assert_eq!(args, vec![OsString::from("imv"), OsString::from("/home/thomas/my pic.png")]);
    }
}
