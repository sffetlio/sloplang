//! Lowers S-expressions to the AST. One error per top-level form; the remaining forms
//! are still parsed so a single run reports as much as possible.

use crate::ast::*;
use crate::diag::Diag;
use crate::sexp::{flat, Sx, SxK};

pub const RESERVED: &[&str] = &[
    "fn", "struct", "test", "prop", "let", "var", "set", "if", "while", "for", "pfor", "each",
    "do", "ret", "break", "continue", "and", "or", "not", "as", "mk", "alloc", "get", "put", "len",
    "free", "freeze", "xadd", "cas", "spawn", "join", "go", "group", "wait", "chan", "send",
    "recv", "close", "print", "clock", "assert", "true", "false", "eff", "doc", "pre", "post",
    "buf", "ro", "task", "i8", "i16", "i32", "i64", "u8", "u16", "u32", "u64", "f32", "f64",
    "bool", "unit", "str", "io", "mem", "conc", "else", "neg",
];

type R<T> = Result<T, Diag>;

pub struct Parser {
    next_id: usize,
}

fn err(s: &Sx, code: &'static str, msg: impl Into<String>) -> Diag {
    Diag::err(code, s.line, s.col, msg)
}

fn shape(s: &Sx, want: &str) -> Diag {
    err(s, "E0101", format!("malformed form, expected {}", want)).fix(want.to_string())
}

pub fn check_name(s: &Sx) -> R<String> {
    let n = s.sym().ok_or_else(|| err(s, "E0103", "expected a name"))?;
    if RESERVED.contains(&n) {
        return Err(err(s, "E0104", format!("'{}' is reserved", n)).fix(format!("{}_", n)));
    }
    let b = n.as_bytes();
    let ok = b[0].is_ascii_lowercase()
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'_');
    if !ok {
        let fixed: String = n
            .to_ascii_lowercase()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect();
        return Err(err(s, "E0103", format!("invalid name '{}'", n)).fix(fixed));
    }
    Ok(n.to_string())
}

fn check_struct_name(s: &Sx) -> R<String> {
    let n = s
        .sym()
        .ok_or_else(|| err(s, "E0103", "expected a struct name"))?;
    let b = n.as_bytes();
    if !(b[0].is_ascii_uppercase() && b.iter().all(|c| c.is_ascii_alphanumeric())) {
        return Err(err(s, "E0103", format!("invalid struct name '{}'", n)));
    }
    Ok(n.to_string())
}

pub fn parse_ty(s: &Sx) -> R<Ty> {
    match &s.k {
        SxK::Sym(n) => {
            if let Some(t) = Ty::from_name(n) {
                return Ok(t);
            }
            if n.as_bytes()[0].is_ascii_uppercase() {
                return Ok(Ty::Struct(check_struct_name(s)?));
            }
            Err(err(s, "E0106", format!("unknown type '{}'", n)))
        }
        SxK::List(l) if l.len() == 2 => {
            let inner = Box::new(parse_ty(&l[1])?);
            match l[0].sym() {
                Some("buf") => Ok(Ty::Buf(inner)),
                Some("ro") => Ok(Ty::Ro(inner)),
                Some("chan") => Ok(Ty::Chan(inner)),
                Some("task") => Ok(Ty::Task(inner)),
                Some("group") => Ok(Ty::Group(inner)),
                _ => Err(err(
                    s,
                    "E0106",
                    "unknown type constructor; expected buf, ro, chan, task or group",
                )),
            }
        }
        _ => Err(err(s, "E0106", "invalid type")),
    }
}

fn eff(s: &Sx) -> R<Eff> {
    match s.sym() {
        Some("io") => Ok(Eff::Io),
        Some("mem") => Ok(Eff::Mem),
        Some("conc") => Ok(Eff::Conc),
        _ => Err(err(s, "E0101", "unknown effect; expected io, mem or conc")),
    }
}

impl Parser {
    fn mk(&mut self, s: &Sx, k: K) -> Expr {
        self.next_id += 1;
        Expr {
            id: self.next_id,
            line: s.line,
            col: s.col,
            k,
        }
    }

    fn b(&mut self, s: &Sx) -> R<B> {
        Ok(Box::new(self.expr(s)?))
    }

    fn body(&mut self, items: &[Sx]) -> R<Vec<Expr>> {
        items.iter().map(|x| self.expr(x)).collect()
    }

    fn call_parts(&mut self, s: &Sx) -> R<(String, Vec<Expr>)> {
        let l = s
            .list()
            .filter(|l| !l.is_empty())
            .ok_or_else(|| shape(s, "(f args...)"))?;
        let name = check_name(&l[0])?;
        Ok((name, self.body(&l[1..])?))
    }

    pub fn expr(&mut self, s: &Sx) -> R<Expr> {
        let l = match &s.k {
            SxK::Int(v, suf) => {
                let t = suf.as_deref().and_then(Ty::from_name);
                return Ok(self.mk(s, K::Int(*v, t)));
            }
            SxK::Float(v, suf) => {
                let t = suf.as_deref().and_then(Ty::from_name);
                return Ok(self.mk(s, K::Float(*v, t)));
            }
            SxK::Str(v) => return Ok(self.mk(s, K::Str(v.clone()))),
            SxK::Sym(n) => {
                return match n.as_str() {
                    "true" => Ok(self.mk(s, K::Bool(true))),
                    "false" => Ok(self.mk(s, K::Bool(false))),
                    "ret" => Ok(self.mk(s, K::Var("ret".into()))),
                    _ => {
                        let n = check_name(s)?;
                        Ok(self.mk(s, K::Var(n)))
                    }
                };
            }
            SxK::List(l) => l,
        };
        if l.is_empty() {
            return Err(shape(s, "a non-empty form"));
        }
        let head = match l[0].sym() {
            Some(h) => h,
            None => return Err(err(s, "E0101", "form head must be a symbol")),
        };
        let a = &l[1..];
        let n = a.len();
        if let Some(op) = Op::from_sym(head) {
            if op == Op::Sub && n == 1 {
                let x = self.b(&a[0])?;
                return Ok(self.mk(s, K::Neg(x)));
            }
            if n != 2 {
                return Err(shape(s, &format!("({} a b)", head)));
            }
            let (x, y) = (self.b(&a[0])?, self.b(&a[1])?);
            return Ok(self.mk(s, K::Bin(op, x, y)));
        }
        if let Some(p) = Prim::from_sym(head) {
            let args = self.body(a)?;
            return Ok(self.mk(s, K::Prim(p, args, flat(s))));
        }
        let k = match head {
            "let" | "var" => {
                let mutable = head == "var";
                let (ty, init) = match n {
                    2 => (None, &a[1]),
                    3 => (Some(parse_ty(&a[1])?), &a[2]),
                    _ => return Err(shape(s, &format!("({} name [type] value)", head))),
                };
                K::Let {
                    name: check_name(&a[0])?,
                    mutable,
                    ty,
                    init: self.b(init)?,
                }
            }
            "set" if n == 2 => K::Set(self.b(&a[0])?, self.b(&a[1])?),
            "if" if n == 2 || n == 3 => {
                let c = self.b(&a[0])?;
                let t = self.b(&a[1])?;
                let e = if n == 3 { Some(self.b(&a[2])?) } else { None };
                K::If(c, t, e)
            }
            "while" if n >= 1 => K::While(self.b(&a[0])?, self.body(&a[1..])?),
            "for" | "pfor" if n >= 3 => {
                let v = check_name(&a[0])?;
                let lo = self.b(&a[1])?;
                let hi = self.b(&a[2])?;
                let body = self.body(&a[3..])?;
                if head == "for" {
                    K::For(v, lo, hi, body)
                } else {
                    K::Pfor(v, lo, hi, body)
                }
            }
            "each" if n >= 2 => K::Each(check_name(&a[0])?, self.b(&a[1])?, self.body(&a[2..])?),
            "do" if n >= 1 => K::Do(self.body(a)?),
            "ret" if n <= 1 => K::Ret(if n == 1 { Some(self.b(&a[0])?) } else { None }),
            "break" if n == 0 => K::Break,
            "continue" if n == 0 => K::Continue,
            "and" if n == 2 => K::And(self.b(&a[0])?, self.b(&a[1])?),
            "or" if n == 2 => K::Or(self.b(&a[0])?, self.b(&a[1])?),
            "not" if n == 1 => K::Not(self.b(&a[0])?),
            "as" if n == 2 => K::As(parse_ty(&a[0])?, self.b(&a[1])?),
            "mk" if n >= 1 => K::Mk(check_struct_name(&a[0])?, self.body(&a[1..])?),
            "." if n == 2 => K::Field(self.b(&a[0])?, check_name(&a[1])?),
            "spawn" if n == 1 => {
                let (f, args) = self.call_parts(&a[0])?;
                K::Spawn(f, args)
            }
            "go" if n == 2 => {
                let g = self.b(&a[0])?;
                let (f, args) = self.call_parts(&a[1])?;
                K::Go(g, f, args)
            }
            "alloc" if n == 2 => K::Alloc(parse_ty(&a[0])?, self.b(&a[1])?),
            "chan" if n == 2 => K::Chan(parse_ty(&a[0])?, self.b(&a[1])?),
            "group" if n == 1 => K::Group(parse_ty(&a[0])?),
            "set" | "if" | "while" | "for" | "pfor" | "each" | "do" | "ret" | "break"
            | "continue" | "and" | "or" | "not" | "as" | "mk" | "." | "spawn" | "go" | "alloc"
            | "chan" | "group" => {
                return Err(shape(s, &usage(head)));
            }
            "fn" | "struct" | "test" | "prop" | "eff" | "doc" | "pre" | "post" => {
                return Err(err(s, "E0101", format!("'{}' is not an expression", head)));
            }
            _ => {
                let f = check_name(&l[0])?;
                K::Call(f, self.body(a)?)
            }
        };
        Ok(self.mk(s, k))
    }

    fn func(&mut self, s: &Sx, l: &[Sx]) -> R<FnDef> {
        let want = "(fn name ((p type)...) ret [(eff ...)] [(doc \"...\")] [(pre e)...] [(post e)...] body...)";
        if l.len() < 5 {
            return Err(shape(s, want));
        }
        let name = check_name(&l[1])?;
        let mut params = vec![];
        for p in l[2]
            .list()
            .ok_or_else(|| shape(&l[2], "((name type)...)"))?
        {
            match p.list() {
                Some([n, t]) => params.push((check_name(n)?, parse_ty(t)?)),
                _ => return Err(shape(p, "(name type)")),
            }
        }
        let ret = parse_ty(&l[3])?;
        let mut f = FnDef {
            name,
            params,
            ret,
            effs: vec![],
            doc: None,
            pre: vec![],
            post: vec![],
            body: vec![],
            line: s.line,
            col: s.col,
        };
        let mut i = 4;
        while i < l.len() {
            let m = &l[i];
            let ml = match m.list() {
                Some(ml) if !ml.is_empty() => ml,
                _ => break,
            };
            match m.head() {
                Some("eff") => {
                    for e in &ml[1..] {
                        let e = eff(e)?;
                        if !f.effs.contains(&e) {
                            f.effs.push(e);
                        }
                    }
                    f.effs.sort();
                }
                Some("doc") => match ml {
                    [_, d] => match &d.k {
                        SxK::Str(t) => f.doc = Some(t.clone()),
                        _ => return Err(shape(m, "(doc \"text\")")),
                    },
                    _ => return Err(shape(m, "(doc \"text\")")),
                },
                Some("pre") | Some("post") => {
                    if ml.len() != 2 {
                        return Err(shape(m, "(pre expr)"));
                    }
                    let c = Contract {
                        e: self.expr(&ml[1])?,
                        text: flat(&ml[1]),
                    };
                    if m.head() == Some("pre") {
                        f.pre.push(c)
                    } else {
                        f.post.push(c)
                    }
                }
                _ => break,
            }
            i += 1;
        }
        if i == l.len() {
            return Err(err(s, "E0101", "fn body is empty").fix(want));
        }
        f.body = self.body(&l[i..])?;
        Ok(f)
    }

    fn item(&mut self, s: &Sx, p: &mut Program) -> R<()> {
        let l = s
            .list()
            .ok_or_else(|| err(s, "E0102", "expected a top-level form"))?;
        match s.head() {
            Some("fn") => p.fns.push(self.func(s, l)?),
            Some("struct") => {
                if l.len() < 3 {
                    return Err(shape(s, "(struct Name (field type)...)"));
                }
                let name = check_struct_name(&l[1])?;
                let mut fields = vec![];
                for f in &l[2..] {
                    if f.head() == Some("doc") {
                        continue;
                    }
                    match f.list() {
                        Some([n, t]) => fields.push((check_name(n)?, parse_ty(t)?)),
                        _ => return Err(shape(f, "(field type)")),
                    }
                }
                p.structs.push(StructDef {
                    name,
                    fields,
                    line: s.line,
                    col: s.col,
                });
            }
            Some("test") => {
                if l.len() < 3 {
                    return Err(shape(s, "(test name body...)"));
                }
                p.tests.push(TestDef {
                    name: check_name(&l[1])?,
                    body: self.body(&l[2..])?,
                    line: s.line,
                    col: s.col,
                });
            }
            Some("prop") => {
                let want = "(prop name ((x int-type lo hi)...) body... bool-expr)";
                if l.len() < 4 {
                    return Err(shape(s, want));
                }
                let mut vars = vec![];
                for v in l[2].list().ok_or_else(|| shape(&l[2], want))? {
                    match v.list() {
                        Some([n, t, lo, hi]) => {
                            let ty = parse_ty(t)?;
                            let (SxK::Int(lo, _), SxK::Int(hi, _)) = (&lo.k, &hi.k) else {
                                return Err(shape(
                                    v,
                                    "(x int-type lo hi) with integer literal bounds",
                                ));
                            };
                            if !ty.is_int() || lo > hi || *lo < ty.range().0 || *hi > ty.range().1 {
                                return Err(err(v, "E0106", "prop variables need an integer type and lo <= hi within its range"));
                            }
                            vars.push((check_name(n)?, ty, *lo, *hi));
                        }
                        _ => return Err(shape(v, "(x int-type lo hi)")),
                    }
                }
                p.props.push(PropDef {
                    name: check_name(&l[1])?,
                    vars,
                    body: self.body(&l[3..])?,
                    line: s.line,
                    col: s.col,
                });
            }
            _ => {
                return Err(
                    err(s, "E0102", "unknown top-level form").fix("fn | struct | test | prop")
                )
            }
        }
        Ok(())
    }
}

fn usage(head: &str) -> String {
    match head {
        "let" => "(let name [type] value)",
        "var" => "(var name [type] value)",
        "set" => "(set place value)",
        "if" => "(if cond then [else])",
        "while" => "(while cond body...)",
        "for" => "(for i lo hi body...)",
        "pfor" => "(pfor i lo hi body...)",
        "each" => "(each x chan body...)",
        "do" => "(do expr...)",
        "ret" => "(ret [value])",
        "and" | "or" => "(and a b)",
        "not" => "(not a)",
        "as" => "(as type value)",
        "mk" => "(mk Struct field-values...)",
        "." => "(. value field)",
        "spawn" => "(spawn (f args...))",
        "go" => "(go group (f args...))",
        "alloc" => "(alloc type count)",
        "chan" => "(chan type capacity)",
        "group" => "(group type)",
        _ => "(break) / (continue)",
    }
    .to_string()
}

pub fn parse(forms: &[Sx]) -> (Program, Vec<Diag>) {
    let mut p = Program::default();
    let mut ps = Parser { next_id: 0 };
    let mut diags = vec![];
    for f in forms {
        if let Err(d) = ps.item(f, &mut p) {
            diags.push(d);
        }
    }
    (p, diags)
}
