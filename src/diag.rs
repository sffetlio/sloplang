//! Machine-readable diagnostics. Every diagnostic is one JSON object per line.

#[derive(Clone, Copy, Debug, PartialEq)]
#[allow(dead_code)]
pub enum Sev {
    Error,
    Warning,
}

#[derive(Clone, Debug)]
pub struct Diag {
    pub sev: Sev,
    pub code: &'static str,
    pub line: u32,
    pub col: u32,
    pub msg: String,
    pub fix: Option<String>,
}

impl Diag {
    pub fn err(code: &'static str, line: u32, col: u32, msg: impl Into<String>) -> Diag {
        Diag {
            sev: Sev::Error,
            code,
            line,
            col,
            msg: msg.into(),
            fix: None,
        }
    }
    pub fn fix(mut self, f: impl Into<String>) -> Diag {
        self.fix = Some(f.into());
        self
    }
    pub fn json(&self, file: &str) -> String {
        let sev = match self.sev {
            Sev::Error => "error",
            Sev::Warning => "warning",
        };
        let mut s = format!(
            "{{\"sev\":\"{}\",\"code\":\"{}\",\"file\":{},\"line\":{},\"col\":{},\"msg\":{}",
            sev,
            self.code,
            json_str(file),
            self.line,
            self.col,
            json_str(&self.msg)
        );
        if let Some(f) = &self.fix {
            s.push_str(",\"fix\":");
            s.push_str(&json_str(f));
        }
        s.push('}');
        s
    }
}

pub fn json_str(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\t' => o.push_str("\\t"),
            '\r' => o.push_str("\\r"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

/// Levenshtein distance, used for "did you mean" fixes.
pub fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut cur = vec![i; b.len() + 1];
        for j in 1..=b.len() {
            let c = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + c);
        }
        prev = cur;
    }
    prev[b.len()]
}

pub fn closest<'a>(name: &str, cands: impl Iterator<Item = &'a str>) -> Option<&'a str> {
    let mut best: Option<(&str, usize)> = None;
    for c in cands {
        let d = edit_distance(name, c);
        if d <= 2.max(name.len() / 3) && best.map_or(true, |(_, bd)| d < bd) {
            best = Some((c, d));
        }
    }
    best.map(|b| b.0)
}

/// Stable error and trap codes. `slopc explain CODE` prints the entry.
pub const CODES: &[(&str, &str)] = &[
    ("E0001", "unterminated string literal"),
    ("E0002", "malformed number or invalid character in token"),
    ("E0003", "unbalanced parenthesis"),
    ("E0004", "line comments are not part of the language; attach documentation with (doc \"...\") on fn or struct"),
    ("E0101", "malformed form: wrong shape for this keyword"),
    ("E0102", "unknown top-level form; expected fn, struct, test or prop"),
    ("E0103", "invalid name: values and functions are [a-z][a-z0-9_]*, structs are [A-Z][A-Za-z0-9]*"),
    ("E0104", "reserved word used as a name"),
    ("E0105", "duplicate definition"),
    ("E0106", "invalid type"),
    ("E0201", "type mismatch"),
    ("E0202", "unknown identifier"),
    ("E0203", "unknown function"),
    ("E0204", "wrong number of arguments"),
    ("E0205", "unknown struct or field"),
    ("E0206", "assignment to immutable binding; declare with var"),
    ("E0207", "shadowing is forbidden: every name in scope is unique"),
    ("E0208", "operator not defined for these operand types"),
    ("E0209", "break/continue outside of a loop"),
    ("E0210", "main must be (fn main () i64 ...) or (fn main () unit ...)"),
    ("E0211", "recursive struct"),
    ("E0212", "invalid element type: buffers, channels and groups hold scalars or structs (channels may also hold buffers)"),
    ("E0213", "if without else must have unit type"),
    ("E0214", "invalid cast"),
    ("E0215", "control flow not allowed here (ret in test/pfor, break in pfor, nested pfor, let outside a block)"),
    ("E0251", "effect not declared: the function performs an effect missing from its (eff ...) list"),
    ("E0252", "contracts must be pure: no effects inside pre/post"),
    ("E0301", "use of a linear value (buf/task/group) after it was moved"),
    ("E0302", "a linear value declared outside a loop cannot be moved inside the loop"),
    ("E0303", "task or group never joined: structured concurrency requires every task to be joined in the scope that owns it"),
    ("E0305", "pfor buffer access is not provably disjoint: a buffer written in pfor may only be accessed at the loop index; atomics may not mix with plain access"),
    ("E0304", "pfor body cannot assign variables declared outside it; use a buffer slot or an atomic"),
    ("E0306", "linear value (task/group) discarded"),
    ("E0307", "task or group consumed on some branches only"),
    ("E0308", "cannot move a borrowed buffer parameter; only functions returning buf/task/group own their buffer parameters"),
    ("T0001", "trap: integer overflow"),
    ("T0002", "trap: division by zero"),
    ("T0003", "trap: index out of bounds"),
    ("T0004", "trap: precondition failed"),
    ("T0005", "trap: postcondition failed"),
    ("T0006", "trap: assert failed"),
    ("T0007", "trap: recv on closed and empty channel"),
    ("T0008", "trap: send on closed channel"),
    ("T0009", "trap: shift amount out of range"),
    ("T0010", "trap: invalid allocation size"),
    ("T0011", "trap: cast out of range"),
];
