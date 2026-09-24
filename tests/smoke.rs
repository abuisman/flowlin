//! GUI smoke test: runs the real binary under Xvfb against a generated
//! fixture tree (nested folders, a corrupt file, a file with a wrong
//! extension, an animation) and checks the counts it reports.
//! Skipped when `xvfb-run` is not installed.

use std::path::Path;
use std::process::Command;

#[path = "../examples/gen_fixtures.rs"]
#[allow(dead_code)]
mod gen_fixtures;

#[test]
fn smoke_under_xvfb() {
    if Command::new("xvfb-run").arg("--help").output().is_err() {
        eprintln!("xvfb-run not found; skipping GUI smoke test");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let total = gen_fixtures::generate(dir.path(), 4);
    let out = Command::new("timeout")
        .args(["120", "xvfb-run", "-a", "dbus-run-session", "--"])
        .arg(env!("CARGO_BIN_EXE_flowlin"))
        .arg(dir.path())
        .env("FLOWLIN_SMOKE_TEST", "1")
        .env("GDK_BACKEND", "x11")
        .output()
        .expect("run flowlin");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("SMOKE ok"),
        "smoke test did not finish:\n{stdout}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let flat = count_files(dir.path(), false);
    let recursive = count_files(dir.path(), true);
    assert!(stdout.contains(&format!("SMOKE flat={flat} ")), "{stdout}");
    assert!(stdout.contains(&format!("SMOKE recursive={recursive} ")), "{stdout}");
    // Every generated file except notes.txt is listed (corrupt ones included).
    assert_eq!(recursive, total);
}

fn count_files(dir: &Path, recursive: bool) -> usize {
    let mut n = 0;
    for e in std::fs::read_dir(dir).unwrap() {
        let e = e.unwrap();
        let p = e.path();
        if p.is_dir() {
            if recursive {
                n += count_files(&p, true);
            }
        } else if p.extension().is_some_and(|x| x != "txt") {
            n += 1;
        }
    }
    n
}
