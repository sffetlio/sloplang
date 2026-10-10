//! C11 (GNU dialect) backend. Every sloplang expression lowers to one C expression,
//! using GNU statement expressions for blocks. Output depends only on the input
//! program, so builds are reproducible.

use crate::ast::*;
use crate::check::Info;
use crate::diag::json_str;
use std::collections::{BTreeSet, HashMap};

pub const RUNTIME: &str = include_str!("runtime.h");

struct G<'a> {
    info: &'a Info,
    fns: HashMap<&'a str, &'a FnDef>,
    lifted: String,
    tmp: usize,
    locals: Vec<Vec<(String, Ty)>>,
}

pub fn cty(t: &Ty) -> String {
    match t {
        Ty::I8 => "int8_t".into(),
        Ty::I16 => "int16_t".into(),
        Ty::I32 => "int32_t".into(),
        Ty::I64 => "int64_t".into(),
        Ty::U8 => "uint8_t".into(),
        Ty::U16 => "uint16_t".into(),
        Ty::U32 => "uint32_t".into(),
        Ty::U64 => "uint64_t".into(),
        Ty::F32 => "float".into(),
        Ty::F64 => "double".into(),
        Ty::Bool => "bool".into(),
        Ty::Unit | Ty::Never => "sl_unit".into(),
        Ty::Str => "const char *".into(),
        Ty::Buf(_) | Ty::Ro(_) => "sl_buf".into(),
        Ty::Chan(_) => "sl_chan *".into(),
        Ty::Task(_) => "sl_task *".into(),
        Ty::Group(_) => "sl_group *".into(),
        Ty::Struct(s) => format!("s_{}", s),
    }
}

fn limits(t: &Ty) -> (&'static str, &'static str) {
    match t {
        Ty::I8 => ("INT8_MIN", "INT8_MAX"),
        Ty::I16 => ("INT16_MIN", "INT16_MAX"),
        Ty::I32 => ("INT32_MIN", "INT32_MAX"),
        Ty::I64 => ("INT64_MIN", "INT64_MAX"),
        Ty::U8 => ("0", "UINT8_MAX"),
        Ty::U16 => ("0", "UINT16_MAX"),
        Ty::U32 => ("0", "UINT32_MAX"),
        _ => ("0", "UINT64_MAX"),
    }
}

pub fn int_lit(v: i128, t: &Ty) -> String {
    match t {
        Ty::I64 if v == i64::MIN as i128 => "INT64_MIN".into(),
        Ty::I64 => format!("({}LL)", v),
        Ty::U64 => format!("{}ULL", v),
        Ty::F32 | Ty::F64 => format!("(({}){}.0)", cty(t), v),
        _ => format!("(({}){})", cty(t), v),
    }
}

fn float_lit(v: f64, t: &Ty) -> String {
    if *t == Ty::F32 {
        format!("((float){:?})", v)
    } else {
        format!("({:?})", v)
    }
}

/// C string literal body for `s`, safe to embed in trap JSON as well.
fn msg_lit(s: &str) -> String {
    let j = json_str(s);
    c_str(&j[1..j.len() - 1])
}

fn c_str(s: &str) -> String {
    let mut o = String::from("\"");
    for ch in s.chars() {
        match ch {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\t' => o.push_str("\\t"),
            '?' => o.push_str("\\?"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\{:03o}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

fn triv(e: &Expr) -> bool {
    matches!(
        e.k,
        K::Int(..) | K::Float(..) | K::Bool(_) | K::Str(_) | K::Var(_)
    )
}

fn wrap(pre: String, e: String) -> String {
    if pre.is_empty() {
        e
    } else {
        format!("({{ {}{}; }})", pre, e)
    }
}

fn fnv(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h | 1
}

impl<'a> G<'a> {
    fn ty(&self, e: &Expr) -> Ty {
        self.info.types.get(&e.id).cloned().unwrap_or(Ty::Never)
    }

    fn t(&mut self) -> String {
        self.tmp += 1;
        format!("_t{}", self.tmp)
    }

    /// Evaluates expressions left to right. Trivial operands are pure, so temporaries
    /// are only needed when more than one operand may have side effects.
    fn seq(&mut self, es: &[&Expr]) -> (String, Vec<String>) {
        let heavy = es.iter().filter(|e| !triv(e)).count();
        let mut pre = String::new();
        let mut out = vec![];
        for e in es {
            let c = self.ex(e);
            if heavy > 1 && !triv(e) {
                let t = self.t();
                pre.push_str(&format!("{} {} = {}; ", cty(&self.ty(e)), t, c));
                out.push(t);
            } else {
                out.push(c);
            }
        }
        (pre, out)
    }

    fn push_local(&mut self, n: &str, t: Ty) {
        self.locals.last_mut().unwrap().push((n.to_string(), t));
    }

    fn stmt(&mut self, e: &Expr) -> String {
        if let K::Let { name, init, .. } = &e.k {
            let t = self.ty(e);
            let c = self.ex(init);
            self.push_local(name, t.clone());
            format!("{} l_{} = {}; ", cty(&t), name, c)
        } else {
            format!("{}; ", self.ex(e))
        }
    }

    fn stmts(&mut self, body: &[Expr]) -> String {
        self.locals.push(vec![]);
        let s: String = body.iter().map(|e| self.stmt(e)).collect();
        self.locals.pop();
        s
    }

    fn block(&mut self, body: &[Expr], ty: &Ty) -> String {
        self.locals.push(vec![]);
        let mut s = String::from("({ ");
        for e in body {
            s.push_str(&self.stmt(e));
        }
        if matches!(body.last().map(|e| &e.k), Some(K::Let { .. }))
            || matches!(ty, Ty::Unit | Ty::Never)
        {
            s.push_str("(sl_unit)0; ");
        }
        self.locals.pop();
        s.push_str("})");
        s
    }

    fn place(&mut self, e: &Expr) -> String {
        match &e.k {
            K::Var(n) => format!("l_{}", n),
            K::Field(x, f) => format!("{}.f_{}", self.place(x), f),
            _ => unreachable!(),
        }
    }

    fn branch(&mut self, e: &Expr, dst: &str) -> String {
        let c = self.ex(e);
        if self.ty(e) == Ty::Never || dst.is_empty() {
            format!("{};", c)
        } else {
            format!("{} = {};", dst, c)
        }
    }

    fn buf_ref(&mut self, b: &Expr, i: &Expr, e: &Expr) -> (String, String) {
        let elem = match self.ty(b) {
            Ty::Buf(t) | Ty::Ro(t) => cty(&t),
            _ => "sl_unit".into(),
        };
        let (pre, v) = self.seq(&[b, i]);
        let (bb, ii) = if triv(b) && triv(i) {
            (v[0].clone(), v[1].clone())
        } else {
            let (tb, ti) = (self.t(), self.t());
            return (
                format!(
                    "{}sl_buf {} = {}; int64_t {} = {}; ",
                    pre, tb, v[0], ti, v[1]
                ),
                format!(
                    "(({} *){}.p)[sl_idx({}, {}, {}, {})]",
                    elem, tb, tb, ti, e.line, e.col
                ),
            );
        };
        (
            String::new(),
            format!(
                "(({} *){}.p)[sl_idx({}, {}, {}, {})]",
                elem, bb, bb, ii, e.line, e.col
            ),
        )
    }

    /// Lifts the call `f(args)` into a thread trampoline; returns (prelude, struct-pointer var).
    fn spawn_site(&mut self, f: &str, args: &[Expr]) -> (String, String) {
        let fd = self.fns[f];
        self.tmp += 1;
        let n = self.tmp;
        let mut def = format!("typedef struct {{ {} r;", cty(&fd.ret));
        for (j, (_, t)) in fd.params.iter().enumerate() {
            def.push_str(&format!(" {} a{};", cty(t), j));
        }
        def.push_str(&format!(" }} sp_{}_t;\n", n));
        let call_args: Vec<String> = (0..fd.params.len())
            .map(|j| format!("_s->a{}", j))
            .collect();
        def.push_str(&format!(
            "static void *sp_{}(void *_p) {{ sp_{}_t *_s = _p; _s->r = f_{}({}); return NULL; }}\n",
            n,
            n,
            f,
            call_args.join(", ")
        ));
        let s = format!("_s{}", n);
        let mut pre = format!("sp_{}_t *{} = sl_malloc(sizeof(sp_{}_t)); ", n, s, n);
        for (j, a) in args.iter().enumerate() {
            let c = self.ex(a);
            pre.push_str(&format!("{}->a{} = {}; ", s, j, c));
        }
        self.lifted.push_str(&def);
        (pre, format!("sl_task_start(sp_{}, {})", n, s))
    }

    fn pfor(&mut self, v: &str, lo: &Expr, hi: &Expr, body: &[Expr]) -> String {
        let mut used = BTreeSet::new();
        for b in body {
            b.visit(&mut |e| {
                if let K::Var(n) = &e.k {
                    used.insert(n.clone());
                }
            });
        }
        let caps: Vec<(String, Ty)> = self
            .locals
            .iter()
            .flatten()
            .filter(|(n, _)| used.contains(n))
            .cloned()
            .collect();
        let (pre, b) = self.seq(&[lo, hi]);
        self.tmp += 1;
        let n = self.tmp;
        let saved = std::mem::replace(
            &mut self.locals,
            vec![caps.clone(), vec![(v.to_string(), Ty::I64)]],
        );
        let inner = self.stmts(body);
        self.locals = saved;
        let mut def = String::from("typedef struct { char _pad;");
        for (c, t) in &caps {
            def.push_str(&format!(" {} c_{};", cty(t), c));
        }
        def.push_str(&format!(" }} pf_{}_env;\n", n));
        def.push_str(&format!("static void pf_{}(void *_envp, int64_t _lo, int64_t _hi) {{\n  pf_{}_env *_env = _envp;\n", n, n));
        for (c, t) in &caps {
            def.push_str(&format!("  {} l_{} = _env->c_{};\n", cty(t), c, c));
        }
        def.push_str(&format!(
            "  for (int64_t l_{} = _lo; l_{} < _hi; l_{}++) {{ {}}}\n}}\n",
            v, v, v, inner
        ));
        self.lifted.push_str(&def);
        let init: Vec<String> = std::iter::once("0".to_string())
            .chain(caps.iter().map(|(c, _)| format!("l_{}", c)))
            .collect();
        format!(
            "({{ {}pf_{}_env _e{} = {{ {} }}; sl_pfor(pf_{}, &_e{}, {}, {}); (sl_unit)0; }})",
            pre,
            n,
            n,
            init.join(", "),
            n,
            n,
            b[0],
            b[1]
        )
    }

    fn ex(&mut self, e: &Expr) -> String {
        let (l, c) = (e.line, e.col);
        match &e.k {
            K::Int(v, _) => int_lit(*v, &self.ty(e)),
            K::Float(v, _) => float_lit(*v, &self.ty(e)),
            K::Bool(b) => b.to_string(),
            K::Str(s) => c_str(s),
            K::Var(n) => format!("l_{}", n),
            K::Let { .. } => unreachable!("let outside block"),
            K::Set(p, v) => {
                let vc = self.ex(v);
                format!("({{ {} = {}; (sl_unit)0; }})", self.place(p), vc)
            }
            K::If(cnd, t, el) => {
                let cc = self.ex(cnd);
                let ty = self.ty(e);
                match el {
                    None => format!("({{ if ({}) {{ {}; }} (sl_unit)0; }})", cc, self.ex(t)),
                    Some(el) => {
                        if matches!(ty, Ty::Unit | Ty::Never) {
                            let (a, b) = (self.branch(t, ""), self.branch(el, ""));
                            format!(
                                "({{ if ({}) {{ {} }} else {{ {} }} (sl_unit)0; }})",
                                cc, a, b
                            )
                        } else {
                            let r = self.t();
                            let (a, b) = (self.branch(t, &r), self.branch(el, &r));
                            format!(
                                "({{ {} {}; if ({}) {{ {} }} else {{ {} }} {}; }})",
                                cty(&ty),
                                r,
                                cc,
                                a,
                                b,
                                r
                            )
                        }
                    }
                }
            }
            K::While(cnd, body) => {
                let cc = self.ex(cnd);
                let b = self.stmts(body);
                format!(
                    "({{ while (1) {{ if (!({})) break; {}}} (sl_unit)0; }})",
                    cc, b
                )
            }
            K::For(v, lo, hi, body) => {
                let t = cty(&self.ty(lo));
                let (lc, hc) = (self.ex(lo), self.ex(hi));
                let (tl, th) = (self.t(), self.t());
                self.locals.push(vec![(v.clone(), self.ty(lo))]);
                let b = self.stmts(body);
                self.locals.pop();
                format!(
                    "({{ {t} {tl} = {lc}; {t} {th} = {hc}; for ({t} l_{v} = {tl}; l_{v} < {th}; l_{v}++) {{ {b}}} (sl_unit)0; }})"
                )
            }
            K::Pfor(v, lo, hi, body) => self.pfor(v, lo, hi, body),
            K::Each(x, ch, body) => {
                let et = match self.ty(ch) {
                    Ty::Chan(t) => *t,
                    _ => Ty::Unit,
                };
                let cc = self.ex(ch);
                let tc = self.t();
                self.locals.push(vec![(x.clone(), et.clone())]);
                let b = self.stmts(body);
                self.locals.pop();
                format!(
                    "({{ sl_chan *{tc} = {cc}; {} l_{x}; while (sl_chan_recv({tc}, &l_{x})) {{ {b}}} (sl_unit)0; }})",
                    cty(&et)
                )
            }
            K::Do(body) => {
                let t = self.ty(e);
                self.block(body, &t)
            }
            K::Ret(v) => {
                let vc = match v {
                    Some(v) => self.ex(v),
                    None => "0".into(),
                };
                format!("({{ sl_r = {}; goto sl_exit; (sl_unit)0; }})", vc)
            }
            K::Break => "({ break; (sl_unit)0; })".into(),
            K::Continue => "({ continue; (sl_unit)0; })".into(),
            K::Bin(op, a, b) => {
                let t = self.ty(a);
                let (pre, v) = self.seq(&[a, b]);
                let (x, y) = (&v[0], &v[1]);
                let sym = match op {
                    Op::Add | Op::AddW => "+",
                    Op::Sub | Op::SubW => "-",
                    Op::Mul | Op::MulW => "*",
                    Op::Div => "/",
                    Op::Rem => "%",
                    Op::BAnd => "&",
                    Op::BOr => "|",
                    Op::BXor => "^",
                    Op::Shl => "<<",
                    Op::Shr => ">>",
                    Op::Eq => "==",
                    Op::Ne => "!=",
                    Op::Lt => "<",
                    Op::Le => "<=",
                    Op::Gt => ">",
                    Op::Ge => ">=",
                };
                let r =
                    if op.is_cmp() || t.is_float() || matches!(op, Op::BAnd | Op::BOr | Op::BXor) {
                        format!("({} {} {})", x, sym, y)
                    } else {
                        let name = match op {
                            Op::Add => "add",
                            Op::Sub => "sub",
                            Op::Mul => "mul",
                            Op::Div => "div",
                            Op::Rem => "rem",
                            Op::Shl => "shl",
                            Op::Shr => "shr",
                            Op::AddW => "addw",
                            Op::SubW => "subw",
                            _ => "mulw",
                        };
                        if matches!(op, Op::AddW | Op::SubW | Op::MulW) {
                            format!("sl_{}_{}({}, {})", name, t, x, y)
                        } else {
                            format!("sl_{}_{}({}, {}, {}, {})", name, t, x, y, l, c)
                        }
                    };
                wrap(pre, r)
            }
            K::And(a, b) => format!("({} && {})", self.ex(a), self.ex(b)),
            K::Or(a, b) => format!("({} || {})", self.ex(a), self.ex(b)),
            K::Not(a) => format!("(!{})", self.ex(a)),
            K::Neg(a) => {
                let t = self.ty(a);
                let x = self.ex(a);
                if t.is_float() {
                    format!("(-{})", x)
                } else {
                    format!("sl_sub_{}(0, {}, {}, {})", t, x, l, c)
                }
            }
            K::As(t, x) => {
                let xt = self.ty(x);
                let xc = self.ex(x);
                if xt == *t || !t.is_int() || xt == Ty::Bool || xt == Ty::Never {
                    format!("(({}){})", cty(t), xc)
                } else if xt.is_float() {
                    let (lo, hi) = limits(t);
                    format!("SL_FCAST({}, {}, {}, {}, {}, {})", cty(t), xc, lo, hi, l, c)
                } else {
                    let (a, b) = (t.range(), xt.range());
                    if a.0 <= b.0 && a.1 >= b.1 {
                        format!("(({}){})", cty(t), xc)
                    } else {
                        let (lo, hi) = limits(t);
                        format!("SL_CAST({}, {}, {}, {}, {}, {})", cty(t), xc, lo, hi, l, c)
                    }
                }
            }
            K::Mk(s, args) => {
                let refs: Vec<&Expr> = args.iter().collect();
                let (pre, v) = self.seq(&refs);
                wrap(pre, format!("((s_{}){{ {} }})", s, v.join(", ")))
            }
            K::Field(x, f) => format!("({}).f_{}", self.ex(x), f),
            K::Call(f, args) => {
                let refs: Vec<&Expr> = args.iter().collect();
                let (pre, v) = self.seq(&refs);
                wrap(pre, format!("f_{}({})", f, v.join(", ")))
            }
            K::Spawn(f, args) => {
                let (pre, start) = self.spawn_site(f, args);
                format!("({{ {}{}; }})", pre, start)
            }
            K::Go(g, f, args) => {
                let gc = self.ex(g);
                let tg = self.t();
                let (pre, start) = self.spawn_site(f, args);
                format!(
                    "({{ sl_group *{} = {}; {}sl_group_add({}, {}); (sl_unit)0; }})",
                    tg, gc, pre, tg, start
                )
            }
            K::Alloc(t, n) => format!("sl_alloc({}, sizeof({}), {}, {})", self.ex(n), cty(t), l, c),
            K::Chan(t, n) => format!(
                "sl_chan_new({}, sizeof({}), {}, {})",
                self.ex(n),
                cty(t),
                l,
                c
            ),
            K::Group(t) => format!("sl_group_new(sizeof({}))", cty(t)),
            K::Prim(p, a, text) => self.prim(*p, a, text, e),
        }
    }

    fn prim(&mut self, p: Prim, a: &[Expr], text: &str, e: &Expr) -> String {
        let (l, c) = (e.line, e.col);
        match p {
            Prim::Get => {
                let (pre, r) = self.buf_ref(&a[0], &a[1], e);
                wrap(pre, r)
            }
            Prim::Put => {
                let (pre, r) = self.buf_ref(&a[0], &a[1], e);
                let v = self.ex(&a[2]);
                let tv = self.t();
                format!(
                    "({{ {}{} {} = {}; {} = {}; (sl_unit)0; }})",
                    pre,
                    cty(&self.ty(&a[2])),
                    tv,
                    v,
                    r,
                    tv
                )
            }
            Prim::Len => format!("({}).n", self.ex(&a[0])),
            Prim::Free => format!("({{ free(({}).p); (sl_unit)0; }})", self.ex(&a[0])),
            Prim::Freeze => self.ex(&a[0]),
            Prim::Xadd | Prim::Cas => {
                let (pre, r) = self.buf_ref(&a[0], &a[1], e);
                let et = cty(&self.ty(&a[2]));
                let mut s = format!("({{ {}", pre);
                let vals: Vec<String> = a[2..]
                    .iter()
                    .map(|x| {
                        let xc = self.ex(x);
                        let t = self.t();
                        s.push_str(&format!("{} {} = {}; ", et, t, xc));
                        t
                    })
                    .collect();
                if p == Prim::Xadd {
                    s.push_str(&format!(
                        "__atomic_fetch_add(&{}, {}, __ATOMIC_SEQ_CST); }})",
                        r, vals[0]
                    ));
                } else {
                    s.push_str(&format!(
                        "__atomic_compare_exchange_n(&{}, &{}, {}, 0, __ATOMIC_SEQ_CST, __ATOMIC_SEQ_CST); }})",
                        r, vals[0], vals[1]
                    ));
                }
                s
            }
            Prim::Join => {
                let rt = match self.ty(&a[0]) {
                    Ty::Task(t) => cty(&t),
                    _ => "sl_unit".into(),
                };
                let x = self.ex(&a[0]);
                let (tt, tr) = (self.t(), self.t());
                format!(
                    "({{ sl_task *{tt} = {x}; sl_task_join({tt}); {rt} {tr} = *({rt} *){tt}->data; free({tt}->data); free({tt}); {tr}; }})"
                )
            }
            Prim::Wait => format!("sl_group_wait({})", self.ex(&a[0])),
            Prim::Send => {
                let et = cty(&self.ty(&a[1]));
                let (ch, v) = (self.ex(&a[0]), self.ex(&a[1]));
                let (tc, tv) = (self.t(), self.t());
                format!("({{ sl_chan *{tc} = {ch}; {et} {tv} = {v}; sl_chan_send({tc}, &{tv}, {l}, {c}); (sl_unit)0; }})")
            }
            Prim::Recv => {
                let et = cty(&self.ty(e));
                let ch = self.ex(&a[0]);
                let tv = self.t();
                format!("({{ {et} {tv}; if (!sl_chan_recv({ch}, &{tv})) sl_trap(\"T0007\", \"recv on closed channel\", {l}, {c}); {tv}; }})")
            }
            Prim::Close => format!("({{ sl_chan_close({}); (sl_unit)0; }})", self.ex(&a[0])),
            Prim::Print => {
                let mut s = String::from("({ ");
                let mut outs = vec![];
                for x in a {
                    let t = self.ty(x);
                    let xc = self.ex(x);
                    let tv = self.t();
                    s.push_str(&format!("{} {} = {}; ", cty(&t), tv, xc));
                    outs.push((t, tv));
                }
                s.push_str("flockfile(stdout); ");
                for (i, (t, v)) in outs.iter().enumerate() {
                    if i > 0 {
                        s.push_str("putchar(' '); ");
                    }
                    s.push_str(&match t {
                        t if t.is_signed() => format!("printf(\"%lld\", (long long){}); ", v),
                        t if t.is_int() => format!("printf(\"%llu\", (unsigned long long){}); ", v),
                        t if t.is_float() => format!("sl_p_f64((double){}); ", v),
                        Ty::Bool => format!("fputs({} ? \"true\" : \"false\", stdout); ", v),
                        Ty::Str => format!("fputs({}, stdout); ", v),
                        _ => "fputs(\"()\", stdout); ".to_string(),
                    });
                }
                s.push_str("putchar('\\n'); funlockfile(stdout); (sl_unit)0; })");
                s
            }
            Prim::Clock => "sl_clock()".into(),
            Prim::Assert => {
                let cc = self.ex(&a[0]);
                format!(
                    "({{ if (!({})) sl_trap(\"T0006\", {}, {}, {}); (sl_unit)0; }})",
                    cc,
                    msg_lit(&format!("assert failed: {}", text)),
                    l,
                    c
                )
            }
        }
    }

    fn func(&mut self, f: &FnDef) -> String {
        let rt = cty(&f.ret);
        self.locals = vec![f.params.clone()];
        let mut s = format!(
            "static {} f_{}({}) {{\n  {} sl_r;\n",
            rt,
            f.name,
            params(f),
            rt
        );
        if !f.pre.is_empty() || !f.post.is_empty() {
            s.push_str("#ifndef SL_NOCHECK\n");
            for c in &f.pre {
                let msg = msg_lit(&format!("pre {} failed in fn {}", c.text, f.name));
                s.push_str(&format!(
                    "  if (!({})) sl_trap(\"T0004\", {}, {}, {});\n",
                    self.ex(&c.e),
                    msg,
                    c.e.line,
                    c.e.col
                ));
            }
            s.push_str("#endif\n");
        }
        let at = Expr {
            id: 0,
            line: 0,
            col: 0,
            k: K::Break,
        };
        let bt = self
            .info
            .types
            .get(&f.body.last().unwrap_or(&at).id)
            .cloned()
            .unwrap_or(Ty::Never);
        if f.ret == Ty::Unit || bt == Ty::Never {
            s.push_str(&format!("  {}\n", self.stmts(&f.body)));
            if f.ret == Ty::Unit {
                s.push_str("  sl_r = 0;\n");
            }
        } else {
            s.push_str(&format!("  sl_r = {};\n", self.block(&f.body, &f.ret)));
        }
        s.push_str("sl_exit: ;\n");
        if !f.post.is_empty() {
            s.push_str(&format!("#ifndef SL_NOCHECK\n  {{ {} l_ret = sl_r;\n", rt));
            for c in &f.post {
                let msg = msg_lit(&format!("post {} failed in fn {}", c.text, f.name));
                s.push_str(&format!(
                    "  if (!({})) sl_trap(\"T0005\", {}, {}, {});\n",
                    self.ex(&c.e),
                    msg,
                    c.e.line,
                    c.e.col
                ));
            }
            s.push_str("  }\n#endif\n");
        }
        s.push_str("  return sl_r;\n}\n\n");
        s
    }
}

fn params(f: &FnDef) -> String {
    if f.params.is_empty() {
        return "void".into();
    }
    f.params
        .iter()
        .map(|(n, t)| format!("{} l_{}", cty(t), n))
        .collect::<Vec<_>>()
        .join(", ")
}

fn struct_order<'a>(p: &'a Program) -> Vec<&'a StructDef> {
    fn go<'a>(s: &'a StructDef, p: &'a Program, out: &mut Vec<&'a StructDef>) {
        if out.iter().any(|o| o.name == s.name) {
            return;
        }
        for (_, t) in &s.fields {
            if let Ty::Struct(n) = t {
                if let Some(d) = p.structs.iter().find(|d| &d.name == n) {
                    go(d, p, out);
                }
            }
        }
        out.push(s);
    }
    let mut out = vec![];
    for s in &p.structs {
        go(s, p, &mut out);
    }
    out
}

pub fn generate(p: &Program, info: &Info, file: &str, test_mode: bool) -> String {
    let mut g = G {
        info,
        fns: p.fns.iter().map(|f| (f.name.as_str(), f)).collect(),
        lifted: String::new(),
        tmp: 0,
        locals: vec![],
    };
    let mut out = String::from("/* generated by slopc; do not edit */\n");
    out.push_str(&format!("#define SL_FILE {}\n", msg_lit(file)));
    out.push_str(RUNTIME);
    out.push('\n');
    for s in struct_order(p) {
        out.push_str(&format!("typedef struct s_{} {{", s.name));
        for (n, t) in &s.fields {
            out.push_str(&format!(" {} f_{};", cty(t), n));
        }
        out.push_str(&format!(" }} s_{};\n", s.name));
    }
    for f in &p.fns {
        out.push_str(&format!(
            "static {} f_{}({});\n",
            cty(&f.ret),
            f.name,
            params(f)
        ));
    }
    let mut bodies = String::new();
    for f in &p.fns {
        bodies.push_str(&g.func(f));
    }
    if test_mode {
        for (i, t) in p.tests.iter().enumerate() {
            g.locals = vec![];
            let b = g.stmts(&t.body);
            bodies.push_str(&format!("static void t_{}(void) {{ {}}}\n", i, b));
        }
        for (i, t) in p.props.iter().enumerate() {
            let ps: Vec<String> = t
                .vars
                .iter()
                .map(|(n, ty, _, _)| format!("{} l_{}", cty(ty), n))
                .collect();
            g.locals = vec![t
                .vars
                .iter()
                .map(|(n, ty, _, _)| (n.clone(), ty.clone()))
                .collect()];
            let b = g.block(&t.body, &Ty::Bool);
            bodies.push_str(&format!(
                "static bool p_{}({}) {{ return {}; }}\n",
                i,
                ps.join(", "),
                b
            ));
        }
    }
    out.push_str(&g.lifted);
    out.push_str(&bodies);
    if test_mode {
        out.push_str(&harness(p));
    } else if let Some(m) = p.fns.iter().find(|f| f.name == "main") {
        if m.ret == Ty::Unit {
            out.push_str("int main(void) { f_main(); fflush(stdout); return 0; }\n");
        } else {
            out.push_str("int main(void) { int r = (int)f_main(); fflush(stdout); return r; }\n");
        }
    }
    out
}

fn harness(p: &Program) -> String {
    let mut s = String::from("int main(void) {\n  volatile int pass = 0, fail = 0;\n  sl_test_thread = pthread_self();\n");
    for (i, t) in p.tests.iter().enumerate() {
        let name = msg_lit(&t.name);
        s.push_str(&format!(
            "  {{ jmp_buf jb; sl_test_jb = &jb;\n    if (setjmp(jb) == 0) {{ t_{i}(); printf(\"{{\\\"kind\\\":\\\"test\\\",\\\"name\\\":\\\"%s\\\",\\\"ok\\\":true}}\\n\", {name}); pass++; }}\n    else {{ printf(\"{{\\\"kind\\\":\\\"test\\\",\\\"name\\\":\\\"%s\\\",\\\"ok\\\":false,\\\"trap\\\":{{%s}}}}\\n\", {name}, sl_trap_msg); fail++; }}\n    fflush(stdout); }}\n"
        ));
    }
    for (i, t) in p.props.iter().enumerate() {
        let name = msg_lit(&t.name);
        let mut decls = String::new();
        let mut case_fmt = String::new();
        let mut case_args = String::new();
        let mut call = vec![];
        for (j, (n, ty, lo, hi)) in t.vars.iter().enumerate() {
            let (clo, chi) = (int_lit(*lo, ty), int_lit(*hi, ty));
            decls.push_str(&format!(
                "      {ct} v{j} = k == 0 ? {clo} : k == 1 ? {chi} : ({ct})sl_rng_range(&seed, (__int128){clo}, (__int128){chi});\n",
                ct = cty(ty)
            ));
            if j > 0 {
                case_fmt.push(',');
            }
            let f = if ty.is_signed() { "%lld" } else { "%llu" };
            case_fmt.push_str(&format!("\\\"{}\\\":{}", n, f));
            let cast = if ty.is_signed() {
                "long long"
            } else {
                "unsigned long long"
            };
            case_args.push_str(&format!(", ({})v{}", cast, j));
            call.push(format!("v{}", j));
        }
        s.push_str(&format!(
            "  {{ uint64_t seed = {seed}ULL; volatile int ok = 1; int k;\n    for (k = 0; k < 256 && ok; k++) {{\n{decls}      jmp_buf jb; sl_test_jb = &jb;\n      if (setjmp(jb) == 0) {{ if (!p_{i}({call})) {{ ok = 0; printf(\"{{\\\"kind\\\":\\\"prop\\\",\\\"name\\\":\\\"%s\\\",\\\"ok\\\":false,\\\"case\\\":{{{case_fmt}}}}}\\n\", {name}{case_args}); }} }}\n      else {{ ok = 0; printf(\"{{\\\"kind\\\":\\\"prop\\\",\\\"name\\\":\\\"%s\\\",\\\"ok\\\":false,\\\"case\\\":{{{case_fmt}}},\\\"trap\\\":{{%s}}}}\\n\", {name}{case_args}, sl_trap_msg); }}\n    }}\n    if (ok) {{ printf(\"{{\\\"kind\\\":\\\"prop\\\",\\\"name\\\":\\\"%s\\\",\\\"ok\\\":true,\\\"cases\\\":256}}\\n\", {name}); pass++; }} else fail++;\n    fflush(stdout); }}\n",
            seed = fnv(&t.name),
            call = call.join(", ")
        ));
    }
    s.push_str("  sl_test_jb = NULL;\n  printf(\"{\\\"kind\\\":\\\"summary\\\",\\\"pass\\\":%d,\\\"fail\\\":%d}\\n\", pass, fail);\n  return fail ? 1 : 0;\n}\n");
    s
}
