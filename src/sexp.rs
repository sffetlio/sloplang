//! Reader and canonical printer for the S-expression surface syntax.

use crate::diag::Diag;

#[derive(Clone, Debug)]
pub enum SxK {
    List(Vec<Sx>),
    Int(i128, Option<String>),
    Float(f64, Option<String>),
    Str(String),
    Sym(String),
}

#[derive(Clone, Debug)]
pub struct Sx {
    pub k: SxK,
    pub line: u32,
    pub col: u32,
    /// Source spelling of atoms (numbers keep their suffix, strings keep escapes).
    pub raw: String,
}

impl Sx {
    pub fn sym(&self) -> Option<&str> {
        match &self.k {
            SxK::Sym(s) => Some(s),
            _ => None,
        }
    }
    pub fn list(&self) -> Option<&[Sx]> {
        match &self.k {
            SxK::List(v) => Some(v),
            _ => None,
        }
    }
    /// Head symbol of a list form.
    pub fn head(&self) -> Option<&str> {
        self.list().and_then(|l| l.first()).and_then(|h| h.sym())
    }
}

struct Reader {
    c: Vec<char>,
    i: usize,
    line: u32,
    col: u32,
}

impl Reader {
    fn peek(&self) -> Option<char> {
        self.c.get(self.i).copied()
    }
    fn bump(&mut self) -> Option<char> {
        let ch = self.c.get(self.i).copied()?;
        self.i += 1;
        if ch == '\n' {
            self.line += 1;
            self.col = 1;
        } else {
            self.col += 1;
        }
        Some(ch)
    }
    fn skip_ws(&mut self) -> Result<(), Diag> {
        while let Some(ch) = self.peek() {
            if ch.is_whitespace() {
                self.bump();
            } else if ch == ';' {
                return Err(Diag::err(
                    "E0004",
                    self.line,
                    self.col,
                    "line comments are not allowed",
                )
                .fix("(doc \"...\") inside the fn or struct form"));
            } else {
                break;
            }
        }
        Ok(())
    }
    fn read(&mut self) -> Result<Sx, Diag> {
        let (line, col) = (self.line, self.col);
        match self.peek() {
            Some('(') => {
                self.bump();
                let mut items = vec![];
                loop {
                    self.skip_ws()?;
                    match self.peek() {
                        None => return Err(Diag::err("E0003", line, col, "unclosed '('").fix(")")),
                        Some(')') => {
                            self.bump();
                            break;
                        }
                        _ => items.push(self.read()?),
                    }
                }
                Ok(Sx {
                    k: SxK::List(items),
                    line,
                    col,
                    raw: String::new(),
                })
            }
            Some(')') => Err(Diag::err("E0003", line, col, "unexpected ')'")),
            Some('"') => {
                self.bump();
                let mut s = String::new();
                let mut raw = String::from("\"");
                loop {
                    match self.bump() {
                        None => return Err(Diag::err("E0001", line, col, "unterminated string")),
                        Some('"') => break,
                        Some('\\') => {
                            let e = self.bump();
                            raw.push('\\');
                            match e {
                                Some('n') => s.push('\n'),
                                Some('t') => s.push('\t'),
                                Some('\\') => s.push('\\'),
                                Some('"') => s.push('"'),
                                _ => {
                                    return Err(Diag::err(
                                        "E0002",
                                        self.line,
                                        self.col,
                                        "invalid escape; allowed: \\n \\t \\\\ \\\"",
                                    ))
                                }
                            }
                            raw.push(e.unwrap());
                        }
                        Some(ch) => {
                            s.push(ch);
                            raw.push(ch);
                        }
                    }
                }
                raw.push('"');
                Ok(Sx {
                    k: SxK::Str(s),
                    line,
                    col,
                    raw,
                })
            }
            Some(_) => {
                let mut t = String::new();
                while let Some(ch) = self.peek() {
                    if ch.is_whitespace() || ch == '(' || ch == ')' || ch == '"' || ch == ';' {
                        break;
                    }
                    t.push(ch);
                    self.bump();
                }
                atom(t, line, col)
            }
            None => Err(Diag::err("E0003", line, col, "unexpected end of input")),
        }
    }
}

const SUFFIXES: [&str; 10] = [
    "i8", "i16", "i32", "i64", "u8", "u16", "u32", "u64", "f32", "f64",
];

fn atom(t: String, line: u32, col: u32) -> Result<Sx, Diag> {
    let b = t.as_bytes();
    let numeric = b[0].is_ascii_digit() || (b[0] == b'-' && b.len() > 1 && b[1].is_ascii_digit());
    if !numeric {
        return Ok(Sx {
            k: SxK::Sym(t.clone()),
            line,
            col,
            raw: t,
        });
    }
    let bad = || Diag::err("E0002", line, col, format!("malformed number '{}'", t));
    let (neg, body) = if let Some(r) = t.strip_prefix('-') {
        (true, r)
    } else {
        (false, t.as_str())
    };
    let hex = body.starts_with("0x");
    let mut suffix = None;
    let mut digits = body;
    for s in SUFFIXES {
        if hex && s.starts_with('f') {
            continue;
        }
        if let Some(d) = body.strip_suffix(s) {
            if d.chars().last().map_or(false, |c| c.is_ascii_hexdigit()) {
                suffix = Some(s.to_string());
                digits = d;
                break;
            }
        }
    }
    let is_float = !hex && (digits.contains('.') || digits.contains('e'))
        || suffix.as_deref().map_or(false, |s| s.starts_with('f'));
    if is_float {
        if suffix.as_deref().map_or(false, |s| !s.starts_with('f')) {
            return Err(bad());
        }
        if !digits
            .chars()
            .all(|c| c.is_ascii_digit() || c == '.' || c == 'e' || c == '-')
        {
            return Err(bad());
        }
        let v: f64 = digits.parse().map_err(|_| bad())?;
        return Ok(Sx {
            k: SxK::Float(if neg { -v } else { v }, suffix),
            line,
            col,
            raw: t.clone(),
        });
    }
    let v = if hex {
        i128::from_str_radix(&digits[2..], 16).map_err(|_| bad())?
    } else {
        if !digits.chars().all(|c| c.is_ascii_digit()) {
            return Err(bad());
        }
        digits.parse::<i128>().map_err(|_| bad())?
    };
    Ok(Sx {
        k: SxK::Int(if neg { -v } else { v }, suffix),
        line,
        col,
        raw: t.clone(),
    })
}

pub fn read_all(src: &str) -> Result<Vec<Sx>, Diag> {
    let mut r = Reader {
        c: src.chars().collect(),
        i: 0,
        line: 1,
        col: 1,
    };
    let mut out = vec![];
    loop {
        r.skip_ws()?;
        if r.peek().is_none() {
            return Ok(out);
        }
        out.push(r.read()?);
    }
}

/// Single-line rendering.
pub fn flat(s: &Sx) -> String {
    match &s.k {
        SxK::List(items) => {
            let parts: Vec<String> = items.iter().map(flat).collect();
            format!("({})", parts.join(" "))
        }
        _ => s.raw.clone(),
    }
}

const WIDTH: usize = 100;

/// Canonical layout: a form that fits in WIDTH columns stays on one line; otherwise its
/// head and leading atoms stay on the first line and each remaining child gets its own
/// line, indented by two spaces. The output is a pure function of the tree.
pub fn pretty(s: &Sx, indent: usize, out: &mut String) {
    let f = flat(s);
    let items = match &s.k {
        SxK::List(items) if indent + f.len() > WIDTH && !items.is_empty() => items,
        _ => {
            out.push_str(&f);
            return;
        }
    };
    out.push('(');
    // Forms keep their fixed header on the first line: (fn name params ret), (for i lo hi), ...
    let header = match s.head() {
        Some("fn") | Some("for") | Some("pfor") => 4,
        Some("prop") | Some("each") | Some("go") => 3,
        Some("struct") | Some("test") | Some("while") | Some("if") | Some("let") | Some("var")
        | Some("set") => 2,
        _ => 0,
    };
    let mut i = 0;
    while i < items.len()
        && (i == 0 || i < header || (header == 0 && !matches!(items[i].k, SxK::List(_))))
    {
        if i > 0 {
            out.push(' ');
        }
        out.push_str(&flat(&items[i]));
        i += 1;
    }
    for it in &items[i..] {
        out.push('\n');
        out.push_str(&" ".repeat(indent + 2));
        pretty(it, indent + 2, out);
    }
    out.push(')');
}

pub fn format_file(forms: &[Sx]) -> String {
    let mut out = String::new();
    for (i, f) in forms.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        pretty(f, 0, &mut out);
        out.push('\n');
    }
    out
}
