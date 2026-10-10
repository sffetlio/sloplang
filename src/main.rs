mod ast;
mod check;
mod codegen;
mod diag;
mod parse;
mod sexp;

use diag::{json_str, Diag, Sev, CODES};
use std::path::{Path, PathBuf};
use std::process::{exit, Command};

const USAGE: &str = "slopc <command> [args]
  check FILE                 type/effect/ownership check; JSON diagnostics on stdout
  build FILE [-o OUT]        compile to a native binary
  run FILE                   build and run
  test FILE                  run (test ...) and (prop ...) forms; JSON results on stdout
  emit-c FILE                print generated C
  fmt FILE [--write|--check] canonical formatting
  sig FILE                   interface summary: every fn/struct without bodies
  hash FILE                  content hash per top-level form (formatting-insensitive)
  explain CODE               describe a diagnostic or trap code
flags: --no-checks  drop overflow/bounds/contract checks (asserts stay)
env:   CC (default cc), SLOP_THREADS (pfor worker count)";

struct Front {
    forms: Vec<sexp::Sx>,
    prog: ast::Program,
    info: check::Info,
}

fn report(file: &str, diags: &[Diag]) -> bool {
    let mut errors = false;
    for d in diags {
        println!("{}", d.json(file));
        errors |= d.sev == Sev::Error;
    }
    errors
}

fn load(file: &str) -> Vec<sexp::Sx> {
    let src = std::fs::read_to_string(file).unwrap_or_else(|e| {
        println!(
            "{}",
            Diag::err("E0000", 0, 0, format!("cannot read file: {}", e)).json(file)
        );
        exit(2)
    });
    match sexp::read_all(&src) {
        Ok(f) => f,
        Err(d) => {
            report(file, &[d]);
            exit(1)
        }
    }
}

/// Parses and checks; prints diagnostics and exits on error.
fn front(file: &str) -> Front {
    let forms = load(file);
    let (prog, mut diags) = parse::parse(&forms);
    let (info, cd) = check::check(&prog);
    diags.extend(cd);
    diags.sort_by_key(|d| (d.line, d.col));
    if report(file, &diags) {
        exit(1);
    }
    Front { forms, prog, info }
}

fn cc_build(c_src: &str, out: &Path, no_checks: bool) {
    let c_path = out.with_extension("c");
    std::fs::write(&c_path, c_src).expect("write C file");
    let cc = std::env::var("CC").unwrap_or_else(|_| "cc".into());
    let mut cmd = Command::new(&cc);
    cmd.args(["-std=gnu11", "-O2", "-w", "-pthread", "-o"])
        .arg(out)
        .arg(&c_path);
    if no_checks {
        cmd.arg("-DSL_NOCHECK");
    }
    let res = cmd.output().unwrap_or_else(|e| {
        println!(
            "{{\"sev\":\"error\",\"code\":\"E9001\",\"msg\":{}}}",
            json_str(&format!("cannot run {}: {}", cc, e))
        );
        exit(2)
    });
    if !res.status.success() {
        let msg = format!(
            "C compiler failed (internal compiler error): {}",
            String::from_utf8_lossy(&res.stderr)
        );
        println!(
            "{{\"sev\":\"error\",\"code\":\"E9002\",\"msg\":{}}}",
            json_str(&msg)
        );
        exit(2);
    }
    let _ = std::fs::remove_file(&c_path);
}

fn tmp_bin(file: &str, tag: &str) -> PathBuf {
    let stem = Path::new(file)
        .file_stem()
        .unwrap()
        .to_string_lossy()
        .to_string();
    std::env::temp_dir().join(format!("slop-{}-{}-{}", stem, tag, std::process::id()))
}

fn run_bin(bin: &Path) -> i32 {
    let st = Command::new(bin).status().expect("run binary");
    let _ = std::fs::remove_file(bin);
    st.code().unwrap_or(101)
}

fn fnv64(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let no_checks = args.iter().any(|a| a == "--no-checks");
    let pos: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    let (Some(cmd), Some(arg)) = (pos.first(), pos.get(1)) else {
        eprintln!("{}", USAGE);
        exit(2)
    };
    let file = arg.as_str();
    match cmd.as_str() {
        "check" => {
            front(file);
        }
        "emit-c" => {
            let f = front(file);
            print!(
                "{}",
                codegen::generate(&f.prog, &f.info, file, args.iter().any(|a| a == "--test"))
            );
        }
        "build" => {
            let f = front(file);
            if !f.prog.fns.iter().any(|f| f.name == "main") {
                println!(
                    "{}",
                    Diag::err("E0210", 1, 1, "no main function")
                        .fix("(fn main () i64 ...)")
                        .json(file)
                );
                exit(1);
            }
            let out = match args.iter().position(|a| a == "-o") {
                Some(i) => PathBuf::from(&args[i + 1]),
                None => PathBuf::from(Path::new(file).file_stem().unwrap()),
            };
            cc_build(
                &codegen::generate(&f.prog, &f.info, file, false),
                &out,
                no_checks,
            );
        }
        "run" => {
            let f = front(file);
            if !f.prog.fns.iter().any(|f| f.name == "main") {
                println!(
                    "{}",
                    Diag::err("E0210", 1, 1, "no main function")
                        .fix("(fn main () i64 ...)")
                        .json(file)
                );
                exit(1);
            }
            let bin = tmp_bin(file, "run");
            cc_build(
                &codegen::generate(&f.prog, &f.info, file, false),
                &bin,
                no_checks,
            );
            exit(run_bin(&bin));
        }
        "test" => {
            let f = front(file);
            let bin = tmp_bin(file, "test");
            cc_build(
                &codegen::generate(&f.prog, &f.info, file, true),
                &bin,
                no_checks,
            );
            exit(run_bin(&bin));
        }
        "fmt" => {
            let forms = load(file);
            let out = sexp::format_file(&forms);
            if args.iter().any(|a| a == "--write") {
                std::fs::write(file, out).expect("write");
            } else if args.iter().any(|a| a == "--check") {
                let cur = std::fs::read_to_string(file).unwrap();
                if cur != out {
                    println!(
                        "{}",
                        Diag::err("E0005", 1, 1, "file is not canonically formatted")
                            .fix("slopc fmt --write")
                            .json(file)
                    );
                    exit(1);
                }
            } else {
                print!("{}", out);
            }
        }
        "sig" => {
            let f = front(file);
            for s in &f.forms {
                match (s.head(), s.list()) {
                    (Some("struct"), _) => println!("{}", sexp::flat(s)),
                    (Some("fn"), Some(l)) => {
                        let keep: Vec<String> = l
                            .iter()
                            .enumerate()
                            .take_while(|(i, x)| {
                                *i < 4 || matches!(x.head(), Some("eff" | "doc" | "pre" | "post"))
                            })
                            .map(|(_, x)| sexp::flat(x))
                            .collect();
                        println!("({})", keep.join(" "));
                    }
                    _ => {}
                }
            }
        }
        "hash" => {
            for s in load(file) {
                let name = s
                    .list()
                    .and_then(|l| l.get(1))
                    .map(sexp::flat)
                    .unwrap_or_default();
                println!(
                    "{} {} {:016x}",
                    s.head().unwrap_or("?"),
                    name,
                    fnv64(&sexp::flat(&s))
                );
            }
        }
        "explain" => match CODES.iter().find(|(c, _)| c.eq_ignore_ascii_case(file)) {
            Some((c, d)) => println!("{{\"code\":\"{}\",\"explain\":{}}}", c, json_str(d)),
            None => {
                println!("{{\"code\":{},\"explain\":null}}", json_str(file));
                exit(1)
            }
        },
        _ => {
            eprintln!("{}", USAGE);
            exit(2)
        }
    }
}
