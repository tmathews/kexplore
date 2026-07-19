//! Command-line arguments. With no flags kexplore opens as a browser (its
//! original behaviour); the picker flags turn it into a file selector that
//! writes the chosen paths to `--out` (or stdout) and exits 0, or exits 1 if
//! the user cancels.

use std::path::PathBuf;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PickKind {
    OpenFile,
    OpenDir,
    Save,
}

/// Picker configuration, present only when a `--pick-*` / `--save` flag was given.
pub struct Picker {
    pub kind: PickKind,
    /// Whether more than one path may be returned.
    pub multiple: bool,
    /// Where the chosen paths are written; None means stdout.
    pub out: Option<PathBuf>,
    /// Filename prefilled into the draft node (save mode only).
    pub save_name: String,
}

impl Picker {
    /// Title shown at the top of the picker panel.
    pub fn title(&self) -> &'static str {
        match (self.kind, self.multiple) {
            (PickKind::OpenFile, false) => "Select a file",
            (PickKind::OpenFile, true) => "Select files",
            (PickKind::OpenDir, false) => "Choose a folder",
            (PickKind::OpenDir, true) => "Choose folders",
            (PickKind::Save, _) => "Save file",
        }
    }

    /// Label on the confirm button.
    pub fn accept_label(&self) -> &'static str {
        match self.kind {
            PickKind::Save => "Save",
            _ => "Open",
        }
    }
}

pub struct Args {
    pub picker: Option<Picker>,
    /// Directory to open on launch; None means `$HOME`.
    pub start: Option<PathBuf>,
}

pub const USAGE: &str = "\
kexplore — canvas file explorer

Usage:
  kexplore [--start DIR]
  kexplore --pick-file  [--start DIR] [--out PATH]
  kexplore --pick-files [--start DIR] [--out PATH]
  kexplore --pick-dir   [--start DIR] [--out PATH]
  kexplore --pick-dirs  [--start DIR] [--out PATH]
  kexplore --save [NAME] [--start DIR] [--out PATH]

Picker modes write the chosen paths, one per line, to --out (default stdout),
exiting 0 on confirm and 1 on cancel.
";

/// Parse the argument list (excluding argv[0]). Returns the usage string as an
/// error for `--help`, so the caller can print it and exit 0.
pub fn parse<I: Iterator<Item = String>>(args: I) -> Result<Args, String> {
    let argv: Vec<String> = args.collect();
    let mut kind: Option<PickKind> = None;
    let mut multiple = false;
    let mut out: Option<PathBuf> = None;
    let mut start: Option<PathBuf> = None;
    let mut save_name = String::new();

    // `--save` takes an OPTIONAL name, so a following token is consumed only
    // when it is not itself a flag.
    let mut i = 0;
    while i < argv.len() {
        let a = argv[i].as_str();
        let mut want_value = |name: &str| -> Result<String, String> {
            i += 1;
            argv.get(i).cloned().ok_or_else(|| format!("{name} requires a value"))
        };
        match a {
            "--pick-file" => (kind, multiple) = (Some(PickKind::OpenFile), false),
            "--pick-files" => (kind, multiple) = (Some(PickKind::OpenFile), true),
            "--pick-dir" => (kind, multiple) = (Some(PickKind::OpenDir), false),
            "--pick-dirs" => (kind, multiple) = (Some(PickKind::OpenDir), true),
            "--save" => {
                kind = Some(PickKind::Save);
                multiple = false;
                if let Some(next) = argv.get(i + 1) {
                    if !next.starts_with("--") {
                        save_name = next.clone();
                        i += 1;
                    }
                }
            }
            "--out" => out = Some(PathBuf::from(want_value("--out")?)),
            "--start" => start = Some(PathBuf::from(want_value("--start")?)),
            "-h" | "--help" => return Err(USAGE.to_string()),
            other => return Err(format!("unknown argument: {other}\n\n{USAGE}")),
        }
        i += 1;
    }

    let picker = kind.map(|kind| Picker { kind, multiple, out, save_name });
    Ok(Args { picker, start })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_ok(args: &[&str]) -> Args {
        parse(args.iter().map(|s| s.to_string())).unwrap_or_else(|e| panic!("{e}"))
    }

    #[test]
    fn no_args_is_browser_mode() {
        let a = parse_ok(&[]);
        assert!(a.picker.is_none());
        assert!(a.start.is_none());
    }

    #[test]
    fn pick_files_sets_multiple() {
        let a = parse_ok(&["--pick-files"]);
        let p = a.picker.expect("picker");
        assert!(p.multiple);
        assert!(p.kind == PickKind::OpenFile);
    }

    #[test]
    fn save_takes_an_optional_name() {
        let named = parse_ok(&["--save", "report.pdf"]);
        assert_eq!(named.picker.expect("picker").save_name, "report.pdf");
        // A following flag must not be swallowed as the name.
        let bare = parse_ok(&["--save", "--out", "/tmp/o"]);
        let p = bare.picker.expect("picker");
        assert_eq!(p.save_name, "");
        assert_eq!(p.out, Some(PathBuf::from("/tmp/o")));
    }

    /// The exact argument vectors produced by
    /// contrib/kexplore-termfilechooser-wrapper.sh, so the portal contract
    /// cannot drift away from the parser.
    #[test]
    fn parses_what_the_termfilechooser_wrapper_emits() {
        let open = parse_ok(&["--pick-file", "--out", "/tmp/o"]);
        assert!(!open.picker.as_ref().unwrap().multiple);

        let multi = parse_ok(&["--pick-files", "--out", "/tmp/o"]);
        assert!(multi.picker.as_ref().unwrap().multiple);

        let dir = parse_ok(&["--pick-dir", "--out", "/tmp/o"]);
        assert!(dir.picker.as_ref().unwrap().kind == PickKind::OpenDir);

        let save = parse_ok(&[
            "--save",
            "page.html",
            "--start",
            "/home/u/Downloads",
            "--out",
            "/tmp/o",
        ]);
        let p = save.picker.expect("picker");
        assert_eq!(p.save_name, "page.html");
        assert_eq!(p.out, Some(PathBuf::from("/tmp/o")));
        assert_eq!(save.start, Some(PathBuf::from("/home/u/Downloads")));

        // The wrapper emits a bare `--save` when the caller suggested no path;
        // the following flag must not be taken as the filename.
        let bare = parse_ok(&["--save", "--out", "/tmp/o"]);
        let p = bare.picker.expect("picker");
        assert_eq!(p.save_name, "");
        assert_eq!(p.out, Some(PathBuf::from("/tmp/o")));
    }

    #[test]
    fn missing_values_and_unknown_flags_error() {
        assert!(parse(["--out".to_string()].into_iter()).is_err());
        assert!(parse(["--nope".to_string()].into_iter()).is_err());
    }
}
