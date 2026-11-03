//! Decision D-007 (mode M): no code path may reach Pascal's production write API except
//! `pascal-cli place-order --i-am-authorized`. The host appears once in this crate, in the CLI,
//! as the default of `place-order --trade-url`, and the library has no default trade URL.

use std::path::{Path, PathBuf};

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn production_write_host_only_in_the_authorized_cli_path() {
    // Built at run time so this file does not contain the host itself.
    let host = ["trade", "pascal", "trade"].join(".");
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    rust_files(&root.join("src"), &mut files);
    rust_files(&root.join("tests"), &mut files);
    let cli = root.join("src/bin/pascal-cli.rs");
    for f in &files {
        let text = std::fs::read_to_string(f).unwrap();
        let n = text.matches(&host).count();
        if *f == cli {
            assert_eq!(n, 1, "{}: the host once, in PROD_TRADE_URL", f.display());
            let lines: Vec<&str> = text.lines().collect();
            let uses: Vec<usize> = (0..lines.len())
                .filter(|&i| lines[i].contains("PROD_TRADE_URL"))
                .collect();
            assert_eq!(uses.len(), 2, "defined once, used once");
            let at = uses[1];
            assert!(
                lines[at].contains("default_value = PROD_TRADE_URL")
                    && lines[at + 1].trim() == "trade_url: String,"
                    && lines[at.saturating_sub(10)..at]
                        .iter()
                        .any(|l| l.trim() == "PlaceOrder {"),
                "PROD_TRADE_URL is only the default of place-order --trade-url"
            );
        } else {
            assert_eq!(n, 0, "{} names the production write host", f.display());
        }
    }
}
