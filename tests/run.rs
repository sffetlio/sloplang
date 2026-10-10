//! End-to-end tests: every example runs and matches its recorded output, every
//! tests/errors/CODE_*.slop file is rejected with CODE, and formatting/codegen are
//! deterministic.

use std::path::{Path, PathBuf};
use std::process::Command;

fn slopc(args: &[&str]) -> (i32, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_slopc"))
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .args(args)
        .output()
        .expect("run slopc");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
}

fn files(dir: &str, ext: &str) -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut v: Vec<PathBuf> = std::fs::read_dir(root.join(dir))
        .unwrap()
        .map(|e| Path::new(dir).join(e.unwrap().file_name()))
        .filter(|p| p.to_string_lossy().ends_with(ext))
        .collect();
    v.sort();
    v
}

#[test]
fn examples_match_recorded_output() {
    for out in files("examples", ".out") {
        let name = out.to_string_lossy().to_string();
        let (cmd, src) = match name.strip_suffix(".test.out") {
            Some(stem) => ("test", format!("{}.slop", stem)),
            None => ("run", name.replace(".out", ".slop")),
        };
        let (_, got) = slopc(&[cmd, &src]);
        let want =
            std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(&out)).unwrap();
        assert_eq!(got, want, "{} {}", cmd, src);
    }
}

#[test]
fn error_files_report_their_code() {
    let errs = files("tests/errors", ".slop");
    assert!(!errs.is_empty());
    for f in errs {
        let code = f.file_name().unwrap().to_string_lossy()[..5].to_string();
        let (status, out) = slopc(&["check", f.to_str().unwrap()]);
        assert_eq!(status, 1, "{} should fail", f.display());
        assert!(
            out.contains(&format!("\"code\":\"{}\"", code)),
            "{}: expected {}, got {}",
            f.display(),
            code,
            out
        );
    }
}

#[test]
fn examples_are_canonically_formatted() {
    for f in files("examples", ".slop") {
        let (status, out) = slopc(&["fmt", f.to_str().unwrap(), "--check"]);
        assert_eq!(status, 0, "{}", out);
    }
}

#[test]
fn codegen_is_deterministic() {
    for f in files("examples", ".slop") {
        let a = slopc(&["emit-c", f.to_str().unwrap()]).1;
        let b = slopc(&["emit-c", f.to_str().unwrap()]).1;
        assert_eq!(a, b);
    }
}

#[test]
fn no_checks_build_still_runs() {
    let (status, out) = slopc(&["run", "examples/fib.slop", "--no-checks"]);
    assert_eq!(status, 0);
    assert!(out.contains("832040"));
}

#[test]
fn explain_knows_every_code_used_in_tests() {
    for f in files("tests/errors", ".slop") {
        let code = f.file_name().unwrap().to_string_lossy()[..5].to_string();
        let (status, out) = slopc(&["explain", &code]);
        assert_eq!(status, 0, "{}", code);
        assert!(out.contains("explain"));
    }
}
