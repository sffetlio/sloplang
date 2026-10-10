//! Type, effect, ownership and concurrency checking.
//!
//! Ownership model (see docs/spec.md §5):
//! - buf, task and group values are linear: they move, never copy.
//! - task and group must be consumed exactly once (join / wait), in the scope that owns them.
//! - functions returning buf/task/group own their buffer parameters; all other functions borrow.
//! - pfor bodies may write an outer buffer only at the loop index.

use crate::ast::*;
use crate::diag::{closest, Diag};
use std::collections::{BTreeMap, HashMap};

pub struct Info {
    pub types: HashMap<usize, Ty>,
}

struct Var {
    name: String,
    ty: Ty,
    mutable: bool,
    moved: Option<(u32, u32)>,
    loop_depth: usize,
    borrowed: bool,
    line: u32,
    col: u32,
}

#[derive(Default)]
struct Acc {
    get_i: bool,
    get_any: bool,
    put_i: bool,
    atomic: bool,
}

struct PforCtx {
    var: String,
    base_scope: usize,
    loop_depth: usize,
    acc: BTreeMap<String, Acc>,
}

#[derive(Clone, Copy, PartialEq)]
enum CallKind {
    Plain,
    Spawn,
}

type Snap = Vec<Vec<Option<(u32, u32)>>>;

struct Ck<'a> {
    fns: HashMap<String, &'a FnDef>,
    structs: HashMap<String, &'a StructDef>,
    writes: HashMap<String, Vec<bool>>,
    diags: Vec<Diag>,
    types: HashMap<usize, Ty>,
    scopes: Vec<Vec<Var>>,
    loop_depth: usize,
    effs: Option<Vec<Eff>>,
    reported_effs: Vec<Eff>,
    ctx: String,
    ret_ty: Option<Ty>,
    in_contract: bool,
    pfor: Option<PforCtx>,
}

fn compat(a: &Ty, b: &Ty) -> bool {
    a == b || *a == Ty::Never || *b == Ty::Never
}

fn is_lit(e: &Expr) -> bool {
    match &e.k {
        K::Int(_, None) | K::Float(_, None) => true,
        K::Neg(x) => is_lit(x),
        _ => false,
    }
}

fn var_name(e: &Expr) -> Option<&str> {
    match &e.k {
        K::Var(n) => Some(n),
        _ => None,
    }
}

fn scalar(t: &Ty) -> bool {
    t.is_num() || *t == Ty::Bool || matches!(t, Ty::Struct(_))
}

impl<'a> Ck<'a> {
    fn err(&mut self, d: Diag) {
        self.diags.push(d);
    }

    fn lookup(&self, name: &str) -> Option<(usize, usize)> {
        for (si, s) in self.scopes.iter().enumerate().rev() {
            for (vi, v) in s.iter().enumerate().rev() {
                if v.name == name {
                    return Some((si, vi));
                }
            }
        }
        None
    }

    fn visible_names(&self) -> Vec<String> {
        self.scopes
            .iter()
            .flatten()
            .map(|v| v.name.clone())
            .collect()
    }

    fn declare(&mut self, name: &str, ty: Ty, mutable: bool, line: u32, col: u32) {
        if self.lookup(name).is_some() || self.fns.contains_key(name) {
            self.err(
                Diag::err(
                    "E0207",
                    line,
                    col,
                    format!("'{}' shadows an existing name", name),
                )
                .fix(format!("{}_2", name)),
            );
        }
        let ld = self.loop_depth;
        self.scopes.last_mut().unwrap().push(Var {
            name: name.to_string(),
            ty,
            mutable,
            moved: None,
            loop_depth: ld,
            borrowed: false,
            line,
            col,
        });
    }

    fn pop_scope(&mut self, check_leaks: bool) {
        let s = self.scopes.pop().unwrap();
        if check_leaks {
            for v in s {
                let (line, col) = (v.line, v.col);
                if v.ty.must_consume() && v.moved.is_none() {
                    let f = if matches!(v.ty, Ty::Task(_)) {
                        "join"
                    } else {
                        "wait"
                    };
                    self.err(
                        Diag::err(
                            "E0303",
                            line,
                            col,
                            format!("'{}' of type {} is never consumed", v.name, v.ty),
                        )
                        .fix(format!("({} {})", f, v.name)),
                    );
                }
            }
        }
    }

    fn ty_ok(&mut self, t: &Ty, line: u32, col: u32) -> bool {
        match t {
            Ty::Struct(s) => {
                if !self.structs.contains_key(s) {
                    let names: Vec<String> = self.structs.keys().cloned().collect();
                    let mut d = Diag::err("E0205", line, col, format!("unknown struct '{}'", s));
                    if let Some(c) = closest(s, names.iter().map(|s| s.as_str())) {
                        d = d.fix(c);
                    }
                    self.err(d);
                    return false;
                }
                true
            }
            Ty::Buf(e) | Ty::Ro(e) | Ty::Group(e) => {
                if !self.ty_ok(e, line, col) {
                    return false;
                }
                if !scalar(e) {
                    self.err(Diag::err(
                        "E0212",
                        line,
                        col,
                        format!("invalid element type {}", e),
                    ));
                    return false;
                }
                true
            }
            Ty::Chan(e) => {
                if !self.ty_ok(e, line, col) {
                    return false;
                }
                if !(scalar(e) || matches!(**e, Ty::Buf(_))) {
                    self.err(Diag::err(
                        "E0212",
                        line,
                        col,
                        format!("invalid channel element type {}", e),
                    ));
                    return false;
                }
                true
            }
            Ty::Task(e) => self.ty_ok(e, line, col),
            _ => true,
        }
    }

    fn need(&mut self, eff: Eff, e: &Expr) {
        if self.in_contract {
            self.err(Diag::err(
                "E0252",
                e.line,
                e.col,
                format!("effect '{}' inside a contract", eff.name()),
            ));
            return;
        }
        if let Some(a) = &self.effs {
            if !a.contains(&eff) && !self.reported_effs.contains(&eff) {
                let mut want = a.clone();
                want.push(eff);
                want.sort();
                let list: Vec<&str> = want.iter().map(|e| e.name()).collect();
                let msg = format!(
                    "{} performs effect '{}' but does not declare it",
                    self.ctx,
                    eff.name()
                );
                self.reported_effs.push(eff);
                self.err(
                    Diag::err("E0251", e.line, e.col, msg).fix(format!("(eff {})", list.join(" "))),
                );
            }
        }
    }

    fn snap(&self) -> Snap {
        self.scopes
            .iter()
            .map(|s| s.iter().map(|v| v.moved).collect())
            .collect()
    }

    fn restore(&mut self, s: &Snap) {
        for (si, sc) in s.iter().enumerate() {
            for (vi, m) in sc.iter().enumerate() {
                self.scopes[si][vi].moved = *m;
            }
        }
    }

    fn merge(&mut self, a: &Snap, b: &Snap, e: &Expr) {
        for (si, sc) in a.iter().enumerate() {
            for (vi, ma) in sc.iter().enumerate() {
                let mb = b[si][vi];
                let v = &self.scopes[si][vi];
                if v.ty.must_consume() && ma.is_some() != mb.is_some() {
                    let msg = format!("'{}' is consumed on only one branch", v.name);
                    self.err(Diag::err("E0307", e.line, e.col, msg));
                }
                self.scopes[si][vi].moved = ma.or(mb);
            }
        }
    }

    /// Marks a linear variable as moved.
    fn consume(&mut self, e: &Expr) {
        let Some(n) = var_name(e) else { return };
        let Some((si, vi)) = self.lookup(n) else {
            return;
        };
        let v = &self.scopes[si][vi];
        if !v.ty.is_linear() || v.moved.is_some() {
            return;
        }
        if v.borrowed {
            let msg = format!("'{}' is a borrowed buffer parameter and cannot be moved", n);
            self.err(
                Diag::err("E0308", e.line, e.col, msg)
                    .fix("return a buffer from this fn to take ownership of buffer parameters"),
            );
        } else if v.loop_depth < self.loop_depth {
            let msg = format!(
                "'{}' is declared outside this loop and cannot be moved inside it",
                n
            );
            self.err(Diag::err("E0302", e.line, e.col, msg));
        }
        self.scopes[si][vi].moved = Some((e.line, e.col));
    }

    /// If `b` is a buffer variable declared outside the current pfor, returns its name.
    fn pfor_outer_buf(&self, b: &Expr) -> Option<String> {
        let p = self.pfor.as_ref()?;
        let n = var_name(b)?;
        let (si, vi) = self.lookup(n)?;
        if si < p.base_scope && matches!(self.scopes[si][vi].ty, Ty::Buf(_)) {
            Some(n.to_string())
        } else {
            None
        }
    }

    fn mark(&mut self, b: &Expr, idx: Option<&Expr>, write: bool, atomic: bool) {
        let Some(n) = self.pfor_outer_buf(b) else {
            return;
        };
        let p = self.pfor.as_mut().unwrap();
        let at_i = idx.and_then(var_name) == Some(p.var.as_str());
        let a = p.acc.entry(n.clone()).or_default();
        if atomic {
            a.atomic = true;
        } else if write {
            if at_i {
                a.put_i = true;
            } else {
                let msg = format!(
                    "pfor writes '{}' at an index other than the loop variable '{}'",
                    n, p.var
                );
                let d = Diag::err("E0305", b.line, b.col, msg)
                    .fix(format!("index with {} or use xadd", p.var));
                self.err(d);
            }
        } else if at_i {
            a.get_i = true;
        } else {
            a.get_any = true;
        }
    }

    fn ex(&mut self, e: &Expr, expect: Option<&Ty>) -> Ty {
        let t = self.ex_inner(e, expect);
        self.types.insert(e.id, t.clone());
        t
    }

    fn expect_ty(&mut self, e: &Expr, want: &Ty) -> Ty {
        let t = self.ex(e, Some(want));
        if !compat(&t, want) {
            let mut d = Diag::err(
                "E0201",
                e.line,
                e.col,
                format!("expected {}, found {}", want, t),
            );
            if (t.is_num() || t == Ty::Bool) && want.is_num() {
                d = d.fix(format!("(as {} ...)", want));
            }
            self.err(d);
        }
        t
    }

    /// Checks two operands that must share a type; literals adopt the other side's type.
    fn pair(&mut self, a: &Expr, b: &Expr, expect: Option<&Ty>) -> Ty {
        if is_lit(a) && !is_lit(b) {
            let bt = self.ex(b, expect);
            let at = self.expect_ty(a, &bt);
            if bt == Ty::Never {
                at
            } else {
                bt
            }
        } else {
            let at = self.ex(a, expect);
            let bt = self.expect_ty(b, &at);
            if at == Ty::Never {
                bt
            } else {
                at
            }
        }
    }

    fn block(&mut self, body: &[Expr], expect: Option<&Ty>, used: bool, _at: &Expr) -> Ty {
        self.scopes.push(vec![]);
        let mut last = Ty::Unit;
        for (i, e) in body.iter().enumerate() {
            let is_last = i + 1 == body.len();
            if let K::Let {
                name,
                mutable,
                ty,
                init,
            } = &e.k
            {
                let vt = match ty {
                    Some(t) => {
                        self.ty_ok(t, e.line, e.col);
                        self.expect_ty(init, t);
                        t.clone()
                    }
                    None => self.ex(init, None),
                };
                if vt == Ty::Unit || vt == Ty::Never {
                    self.err(Diag::err(
                        "E0201",
                        e.line,
                        e.col,
                        format!("cannot bind a value of type {}", vt),
                    ));
                }
                self.consume(init);
                self.declare(name, vt.clone(), *mutable, e.line, e.col);
                self.types.insert(e.id, vt);
                last = Ty::Unit;
                continue;
            }
            let t = self.ex(e, if is_last { expect } else { None });
            if is_last && used {
                self.consume(e);
            } else if t.must_consume() {
                self.err(
                    Diag::err(
                        "E0306",
                        e.line,
                        e.col,
                        format!("value of type {} is discarded", t),
                    )
                    .fix("bind it with let and join it"),
                );
            }
            last = t;
        }
        self.pop_scope(last != Ty::Never);
        last
    }

    fn place(&mut self, e: &Expr) -> Ty {
        let t = match &e.k {
            K::Var(n) => match self.lookup(n) {
                None => {
                    self.err(Diag::err(
                        "E0202",
                        e.line,
                        e.col,
                        format!("unknown variable '{}'", n),
                    ));
                    Ty::Never
                }
                Some((si, vi)) => {
                    let v = &self.scopes[si][vi];
                    let (mutable, ty) = (v.mutable, v.ty.clone());
                    if !mutable {
                        self.err(
                            Diag::err("E0206", e.line, e.col, format!("'{}' is immutable", n))
                                .fix(format!("(var {} ...)", n)),
                        );
                    }
                    if let Some(p) = &self.pfor {
                        if si < p.base_scope {
                            self.err(Diag::err(
                                "E0304",
                                e.line,
                                e.col,
                                format!("pfor body assigns outer variable '{}'", n),
                            ));
                        }
                    }
                    ty
                }
            },
            K::Field(x, f) => {
                let xt = self.place(x);
                self.field_ty(&xt, f, e)
            }
            _ => {
                self.err(Diag::err(
                    "E0101",
                    e.line,
                    e.col,
                    "set target must be a variable or a (. x field) path",
                ));
                Ty::Never
            }
        };
        self.types.insert(e.id, t.clone());
        t
    }

    fn field_ty(&mut self, xt: &Ty, f: &str, e: &Expr) -> Ty {
        match xt {
            Ty::Struct(s) => {
                let sd = self.structs[s.as_str()];
                match sd.fields.iter().find(|(n, _)| n == f) {
                    Some((_, t)) => t.clone(),
                    None => {
                        let mut d = Diag::err(
                            "E0205",
                            e.line,
                            e.col,
                            format!("struct {} has no field '{}'", s, f),
                        );
                        if let Some(c) = closest(f, sd.fields.iter().map(|(n, _)| n.as_str())) {
                            d = d.fix(c);
                        }
                        self.err(d);
                        Ty::Never
                    }
                }
            }
            Ty::Never => Ty::Never,
            t => {
                self.err(Diag::err(
                    "E0205",
                    e.line,
                    e.col,
                    format!("field access on non-struct type {}", t),
                ));
                Ty::Never
            }
        }
    }

    fn call(&mut self, f: &str, args: &[Expr], e: &Expr, kind: CallKind) -> Ty {
        let Some(fd) = self.fns.get(f).copied() else {
            let names: Vec<String> = self.fns.keys().cloned().collect();
            let mut d = Diag::err("E0203", e.line, e.col, format!("unknown function '{}'", f));
            if let Some(c) = closest(f, names.iter().map(|s| s.as_str())) {
                d = d.fix(c);
            }
            self.err(d);
            for a in args {
                self.ex(a, None);
            }
            return Ty::Never;
        };
        if args.len() != fd.params.len() {
            let sig: Vec<String> = fd
                .params
                .iter()
                .map(|(n, t)| format!("({} {})", n, t))
                .collect();
            let msg = format!(
                "'{}' takes {} arguments, got {}",
                f,
                fd.params.len(),
                args.len()
            );
            self.err(Diag::err("E0204", e.line, e.col, msg).fix(format!(
                "({} {})",
                f,
                sig.join(" ")
            )));
        }
        for (j, a) in args.iter().enumerate() {
            let Some((_, pt)) = fd.params.get(j) else {
                self.ex(a, None);
                continue;
            };
            if let Some(n) = self.pfor_outer_buf(a) {
                if self.writes[f][j] {
                    let msg = format!("'{}' may write buffer '{}' inside pfor", f, n);
                    self.err(Diag::err("E0305", a.line, a.col, msg));
                } else {
                    self.mark(a, None, false, false);
                }
            }
            self.expect_ty(a, pt);
            let moves = pt.must_consume()
                || (matches!(pt, Ty::Buf(_)) && (kind == CallKind::Spawn || fd.ret.is_linear()));
            if moves {
                self.consume(a);
            }
        }
        for eff in fd.effs.clone() {
            self.need(eff, e);
        }
        fd.ret.clone()
    }

    fn buf_elem(&mut self, b: &Expr, allow_ro: bool) -> Ty {
        let bt = self.ex(b, None);
        match bt {
            Ty::Buf(t) => *t,
            Ty::Ro(t) if allow_ro => *t,
            Ty::Never => Ty::Never,
            Ty::Ro(_) => {
                self.err(Diag::err(
                    "E0201",
                    b.line,
                    b.col,
                    "ro buffers are read-only",
                ));
                Ty::Never
            }
            t => {
                self.err(Diag::err(
                    "E0201",
                    b.line,
                    b.col,
                    format!("expected a buffer, found {}", t),
                ));
                Ty::Never
            }
        }
    }

    fn ex_inner(&mut self, e: &Expr, expect: Option<&Ty>) -> Ty {
        match &e.k {
            K::Int(v, suf) => {
                let t = match (suf, expect) {
                    (Some(t), _) => t.clone(),
                    (None, Some(t)) if t.is_int() => t.clone(),
                    (None, Some(t)) if t.is_float() => {
                        self.err(
                            Diag::err(
                                "E0201",
                                e.line,
                                e.col,
                                format!("integer literal where {} is expected", t),
                            )
                            .fix(format!("{}.0", v)),
                        );
                        return t.clone();
                    }
                    _ => Ty::I64,
                };
                let (lo, hi) = t.range();
                if *v < lo || *v > hi {
                    self.err(Diag::err(
                        "E0201",
                        e.line,
                        e.col,
                        format!("literal {} out of range for {}", v, t),
                    ));
                }
                t
            }
            K::Float(_, suf) => match (suf, expect) {
                (Some(t), _) => t.clone(),
                (None, Some(Ty::F32)) => Ty::F32,
                _ => Ty::F64,
            },
            K::Bool(_) => Ty::Bool,
            K::Str(_) => Ty::Str,
            K::Var(n) => match self.lookup(n) {
                None => {
                    let names = self.visible_names();
                    let mut d = Diag::err(
                        "E0202",
                        e.line,
                        e.col,
                        format!("unknown identifier '{}'", n),
                    );
                    if let Some(c) = closest(n, names.iter().map(|s| s.as_str())) {
                        d = d.fix(c);
                    }
                    self.err(d);
                    Ty::Never
                }
                Some((si, vi)) => {
                    let v = &self.scopes[si][vi];
                    if let Some((l, c)) = v.moved {
                        let msg = format!("'{}' was moved at {}:{}", n, l, c);
                        self.err(
                            Diag::err("E0301", e.line, e.col, msg)
                                .fix("use the value handed back by whatever took ownership"),
                        );
                    }
                    self.scopes[si][vi].ty.clone()
                }
            },
            K::Let { .. } => {
                self.err(
                    Diag::err(
                        "E0215",
                        e.line,
                        e.col,
                        "let/var must appear directly in a block",
                    )
                    .fix("(do (let ...) ...)"),
                );
                Ty::Unit
            }
            K::Set(p, v) => {
                let pt = self.place(p);
                self.expect_ty(v, &pt);
                self.consume(v);
                if let Some(n) = var_name(p) {
                    if let Some((si, vi)) = self.lookup(n) {
                        self.scopes[si][vi].moved = None;
                    }
                }
                Ty::Unit
            }
            K::If(c, t, el) => {
                self.expect_ty(c, &Ty::Bool);
                let s0 = self.snap();
                match el {
                    None => {
                        let tt = self.ex(t, None);
                        if tt.must_consume() {
                            self.err(Diag::err(
                                "E0306",
                                t.line,
                                t.col,
                                format!("value of type {} is discarded", tt),
                            ));
                        }
                        let s1 = self.snap();
                        self.restore(&s0);
                        if tt != Ty::Never {
                            self.merge(&s1, &s0, e);
                        }
                        Ty::Unit
                    }
                    Some(el) => {
                        let tt = self.ex(t, expect);
                        self.consume(t);
                        let s1 = self.snap();
                        self.restore(&s0);
                        let hint = if tt != Ty::Never { Some(&tt) } else { expect };
                        let te = self.ex(el, hint);
                        self.consume(el);
                        let s2 = self.snap();
                        match (tt == Ty::Never, te == Ty::Never) {
                            (true, _) => {}
                            (false, true) => self.restore(&s1),
                            _ => self.merge(&s1, &s2, e),
                        }
                        if tt == Ty::Never {
                            te
                        } else if te == Ty::Never {
                            tt
                        } else {
                            if tt != te {
                                self.err(Diag::err(
                                    "E0201",
                                    e.line,
                                    e.col,
                                    format!("if branches differ: {} vs {}", tt, te),
                                ));
                            }
                            tt
                        }
                    }
                }
            }
            K::While(c, body) => {
                self.loop_depth += 1;
                self.expect_ty(c, &Ty::Bool);
                self.block(body, None, false, e);
                self.loop_depth -= 1;
                Ty::Unit
            }
            K::For(v, lo, hi, body) => {
                let t = self.pair(lo, hi, None);
                if !t.is_int() && t != Ty::Never {
                    self.err(Diag::err(
                        "E0201",
                        lo.line,
                        lo.col,
                        format!("for bounds must be integers, found {}", t),
                    ));
                }
                self.loop_depth += 1;
                self.scopes.push(vec![]);
                self.declare(v, t, false, e.line, e.col);
                self.block(body, None, false, e);
                self.scopes.pop();
                self.loop_depth -= 1;
                Ty::Unit
            }
            K::Pfor(v, lo, hi, body) => {
                self.need(Eff::Conc, e);
                if self.pfor.is_some() {
                    self.err(
                        Diag::err("E0215", e.line, e.col, "nested pfor")
                            .fix("use for in the inner loop"),
                    );
                }
                self.expect_ty(lo, &Ty::I64);
                self.expect_ty(hi, &Ty::I64);
                self.loop_depth += 1;
                self.scopes.push(vec![]);
                let saved_ret = self.ret_ty.take();
                let outer = self.pfor.replace(PforCtx {
                    var: v.clone(),
                    base_scope: self.scopes.len() - 1,
                    loop_depth: self.loop_depth,
                    acc: BTreeMap::new(),
                });
                self.declare(v, Ty::I64, false, e.line, e.col);
                self.block(body, None, false, e);
                let ctx = std::mem::replace(&mut self.pfor, outer).unwrap();
                self.ret_ty = saved_ret;
                self.scopes.pop();
                self.loop_depth -= 1;
                for (n, a) in ctx.acc {
                    if a.put_i && (a.get_any || a.atomic) {
                        let msg = format!(
                            "pfor writes '{}' at the loop index but also accesses it elsewhere",
                            n
                        );
                        self.err(Diag::err("E0305", e.line, e.col, msg));
                    } else if a.atomic && (a.get_i || a.get_any) {
                        let msg = format!("pfor mixes atomic and plain access to '{}'", n);
                        self.err(Diag::err("E0305", e.line, e.col, msg));
                    }
                }
                Ty::Unit
            }
            K::Each(x, c, body) => {
                self.need(Eff::Conc, e);
                let ct = self.ex(c, None);
                let elem = match ct {
                    Ty::Chan(t) => *t,
                    Ty::Never => Ty::Never,
                    t => {
                        self.err(Diag::err(
                            "E0201",
                            c.line,
                            c.col,
                            format!("each needs a channel, found {}", t),
                        ));
                        Ty::Never
                    }
                };
                self.loop_depth += 1;
                self.scopes.push(vec![]);
                self.declare(x, elem, false, e.line, e.col);
                self.block(body, None, false, e);
                self.scopes.pop();
                self.loop_depth -= 1;
                Ty::Unit
            }
            K::Do(body) => self.block(body, expect, true, e),
            K::Ret(v) => {
                match self.ret_ty.clone() {
                    None => self.err(Diag::err("E0215", e.line, e.col, "ret is not allowed here")),
                    Some(rt) => {
                        match v {
                            Some(v) => {
                                self.expect_ty(v, &rt);
                                self.consume(v);
                            }
                            None if rt != Ty::Unit => {
                                self.err(Diag::err(
                                    "E0201",
                                    e.line,
                                    e.col,
                                    format!("ret without a value in a fn returning {}", rt),
                                ));
                            }
                            None => {}
                        }
                        let live: Vec<String> = self
                            .scopes
                            .iter()
                            .flatten()
                            .filter(|v| v.ty.must_consume() && v.moved.is_none())
                            .map(|v| v.name.clone())
                            .collect();
                        for n in live {
                            self.err(
                                Diag::err(
                                    "E0303",
                                    e.line,
                                    e.col,
                                    format!("'{}' is not joined before ret", n),
                                )
                                .fix(format!("(join {})", n)),
                            );
                        }
                    }
                }
                Ty::Never
            }
            K::Break | K::Continue => {
                if self.loop_depth == 0 {
                    self.err(Diag::err(
                        "E0209",
                        e.line,
                        e.col,
                        "break/continue outside a loop",
                    ));
                } else if matches!(e.k, K::Break)
                    && self
                        .pfor
                        .as_ref()
                        .map_or(false, |p| p.loop_depth == self.loop_depth)
                {
                    self.err(Diag::err(
                        "E0215",
                        e.line,
                        e.col,
                        "break is not allowed directly in pfor",
                    ));
                }
                Ty::Never
            }
            K::Bin(op, a, b) => {
                let t = if matches!(op, Op::Shl | Op::Shr) {
                    let at = self.ex(a, expect);
                    self.expect_ty(b, &at);
                    at
                } else {
                    self.pair(a, b, if op.is_cmp() { None } else { expect })
                };
                if t == Ty::Never {
                    return if op.is_cmp() { Ty::Bool } else { Ty::Never };
                }
                let ok = match op {
                    Op::Add | Op::Sub | Op::Mul | Op::Div | Op::Lt | Op::Le | Op::Gt | Op::Ge => {
                        t.is_num()
                    }
                    Op::Eq | Op::Ne => t.is_num() || t == Ty::Bool,
                    _ => t.is_int(),
                };
                if !ok {
                    self.err(Diag::err(
                        "E0208",
                        e.line,
                        e.col,
                        format!("operator {:?} is not defined for {}", op, t),
                    ));
                }
                if op.is_cmp() {
                    Ty::Bool
                } else {
                    t
                }
            }
            K::And(a, b) | K::Or(a, b) => {
                self.expect_ty(a, &Ty::Bool);
                self.expect_ty(b, &Ty::Bool);
                Ty::Bool
            }
            K::Not(a) => {
                self.expect_ty(a, &Ty::Bool);
                Ty::Bool
            }
            K::Neg(a) => {
                let t = self.ex(a, expect);
                if !(t.is_float() || t.is_signed() || t == Ty::Never) {
                    self.err(Diag::err(
                        "E0208",
                        e.line,
                        e.col,
                        format!("negation is not defined for {}", t),
                    ));
                }
                t
            }
            K::As(t, x) => {
                self.ty_ok(t, e.line, e.col);
                let xt = self.ex(x, None);
                let ok =
                    xt == *t || xt == Ty::Never || ((xt.is_num() || xt == Ty::Bool) && t.is_num());
                if !ok {
                    self.err(Diag::err(
                        "E0214",
                        e.line,
                        e.col,
                        format!("cannot cast {} to {}", xt, t),
                    ));
                }
                t.clone()
            }
            K::Mk(s, args) => {
                let Some(sd) = self.structs.get(s).copied() else {
                    self.ty_ok(&Ty::Struct(s.clone()), e.line, e.col);
                    for a in args {
                        self.ex(a, None);
                    }
                    return Ty::Never;
                };
                if args.len() != sd.fields.len() {
                    let fs: Vec<String> = sd.fields.iter().map(|(n, _)| n.clone()).collect();
                    let msg = format!("{} has {} fields, got {}", s, sd.fields.len(), args.len());
                    self.err(Diag::err("E0204", e.line, e.col, msg).fix(format!(
                        "(mk {} {})",
                        s,
                        fs.join(" ")
                    )));
                }
                for (a, (_, ft)) in args.iter().zip(sd.fields.iter()) {
                    self.expect_ty(a, ft);
                }
                Ty::Struct(s.clone())
            }
            K::Field(x, f) => {
                let xt = self.ex(x, None);
                self.field_ty(&xt, f, e)
            }
            K::Call(f, args) => self.call(f, args, e, CallKind::Plain),
            K::Spawn(f, args) => {
                self.need(Eff::Conc, e);
                let rt = self.call(f, args, e, CallKind::Spawn);
                if rt == Ty::Never {
                    Ty::Never
                } else {
                    Ty::Task(Box::new(rt))
                }
            }
            K::Go(g, f, args) => {
                self.need(Eff::Conc, e);
                let gt = self.ex(g, None);
                let rt = self.call(f, args, e, CallKind::Spawn);
                match gt {
                    Ty::Group(t) => {
                        if !compat(&t, &rt) {
                            self.err(Diag::err(
                                "E0201",
                                e.line,
                                e.col,
                                format!("group holds {}, '{}' returns {}", t, f, rt),
                            ));
                        }
                    }
                    Ty::Never => {}
                    t => self.err(Diag::err(
                        "E0201",
                        g.line,
                        g.col,
                        format!("expected a group, found {}", t),
                    )),
                }
                Ty::Unit
            }
            K::Alloc(t, n) => {
                self.need(Eff::Mem, e);
                let bt = Ty::Buf(Box::new(t.clone()));
                self.ty_ok(&bt, e.line, e.col);
                self.expect_ty(n, &Ty::I64);
                bt
            }
            K::Chan(t, n) => {
                self.need(Eff::Conc, e);
                let ct = Ty::Chan(Box::new(t.clone()));
                self.ty_ok(&ct, e.line, e.col);
                self.expect_ty(n, &Ty::I64);
                ct
            }
            K::Group(t) => {
                self.need(Eff::Conc, e);
                let gt = Ty::Group(Box::new(t.clone()));
                self.ty_ok(&gt, e.line, e.col);
                gt
            }
            K::Prim(p, args, _) => self.prim(*p, args, e),
        }
    }

    fn prim(&mut self, p: Prim, a: &[Expr], e: &Expr) -> Ty {
        let arity = match p {
            Prim::Get | Prim::Send => 2,
            Prim::Put | Prim::Xadd => 3,
            Prim::Cas => 4,
            Prim::Clock => 0,
            Prim::Print => usize::MAX,
            _ => 1,
        };
        if (arity == usize::MAX && a.is_empty()) || (arity != usize::MAX && a.len() != arity) {
            self.err(Diag::err(
                "E0204",
                e.line,
                e.col,
                format!("wrong number of arguments to {:?}", p).to_lowercase(),
            ));
            for x in a {
                self.ex(x, None);
            }
            return Ty::Never;
        }
        match p {
            Prim::Get => {
                self.mark(&a[0], Some(&a[1]), false, false);
                let t = self.buf_elem(&a[0], true);
                self.expect_ty(&a[1], &Ty::I64);
                t
            }
            Prim::Put => {
                self.mark(&a[0], Some(&a[1]), true, false);
                let t = self.buf_elem(&a[0], false);
                self.expect_ty(&a[1], &Ty::I64);
                self.expect_ty(&a[2], &t);
                Ty::Unit
            }
            Prim::Len => {
                self.buf_elem(&a[0], true);
                Ty::I64
            }
            Prim::Free | Prim::Freeze => {
                if p == Prim::Free {
                    self.need(Eff::Mem, e);
                }
                let t = self.buf_elem(&a[0], false);
                self.consume(&a[0]);
                if p == Prim::Free {
                    Ty::Unit
                } else {
                    Ty::Ro(Box::new(t))
                }
            }
            Prim::Xadd | Prim::Cas => {
                self.mark(&a[0], None, true, true);
                let t = self.buf_elem(&a[0], false);
                if !t.is_int() && t != Ty::Never {
                    self.err(Diag::err(
                        "E0208",
                        e.line,
                        e.col,
                        format!("atomics need an integer buffer, found (buf {})", t),
                    ));
                }
                self.expect_ty(&a[1], &Ty::I64);
                for x in &a[2..] {
                    self.expect_ty(x, &t);
                }
                if p == Prim::Xadd {
                    t
                } else {
                    Ty::Bool
                }
            }
            Prim::Join | Prim::Wait => {
                self.need(Eff::Conc, e);
                let t = self.ex(&a[0], None);
                self.consume(&a[0]);
                match (p, t) {
                    (Prim::Join, Ty::Task(r)) => *r,
                    (Prim::Wait, Ty::Group(r)) => Ty::Buf(r),
                    (_, Ty::Never) => Ty::Never,
                    (_, t) => {
                        let want = if p == Prim::Join { "task" } else { "group" };
                        self.err(Diag::err(
                            "E0201",
                            a[0].line,
                            a[0].col,
                            format!("expected a {}, found {}", want, t),
                        ));
                        Ty::Never
                    }
                }
            }
            Prim::Send | Prim::Recv | Prim::Close => {
                self.need(Eff::Conc, e);
                let elem = match self.ex(&a[0], None) {
                    Ty::Chan(t) => *t,
                    Ty::Never => Ty::Never,
                    t => {
                        self.err(Diag::err(
                            "E0201",
                            a[0].line,
                            a[0].col,
                            format!("expected a channel, found {}", t),
                        ));
                        Ty::Never
                    }
                };
                match p {
                    Prim::Send => {
                        self.expect_ty(&a[1], &elem);
                        self.consume(&a[1]);
                        Ty::Unit
                    }
                    Prim::Recv => elem,
                    _ => Ty::Unit,
                }
            }
            Prim::Print => {
                self.need(Eff::Io, e);
                for x in a {
                    let t = self.ex(x, None);
                    if !(t.is_num() || matches!(t, Ty::Bool | Ty::Str | Ty::Unit | Ty::Never)) {
                        self.err(Diag::err(
                            "E0201",
                            x.line,
                            x.col,
                            format!("cannot print a value of type {}", t),
                        ));
                    }
                }
                Ty::Unit
            }
            Prim::Clock => {
                self.need(Eff::Io, e);
                Ty::I64
            }
            Prim::Assert => {
                self.expect_ty(&a[0], &Ty::Bool);
                Ty::Unit
            }
        }
    }

    fn reset(&mut self, ctx: String, effs: Option<Vec<Eff>>, ret: Option<Ty>) {
        self.ctx = ctx;
        self.effs = effs;
        self.reported_effs.clear();
        self.ret_ty = ret;
        self.loop_depth = 0;
        self.pfor = None;
        self.in_contract = false;
        self.scopes.clear();
    }

    fn check_fn(&mut self, f: &FnDef) {
        self.reset(format!("fn {}", f.name), Some(f.effs.clone()), None);
        let owning = f.ret.is_linear();
        self.scopes.push(vec![]);
        self.ty_ok(&f.ret, f.line, f.col);
        for (n, t) in &f.params {
            self.ty_ok(t, f.line, f.col);
            self.declare(n, t.clone(), false, f.line, f.col);
            self.scopes.last_mut().unwrap().last_mut().unwrap().borrowed =
                matches!(t, Ty::Buf(_)) && !owning;
        }
        self.in_contract = true;
        for c in &f.pre {
            self.expect_ty(&c.e, &Ty::Bool);
        }
        let snap = self.snap();
        self.scopes.push(vec![Var {
            name: "ret".into(),
            ty: f.ret.clone(),
            mutable: false,
            moved: None,
            loop_depth: 0,
            borrowed: false,
            line: f.line,
            col: f.col,
        }]);
        for c in &f.post {
            self.expect_ty(&c.e, &Ty::Bool);
        }
        self.scopes.pop();
        self.restore(&snap);
        self.in_contract = false;
        self.ret_ty = Some(f.ret.clone());
        let unit = f.ret == Ty::Unit;
        let at = Expr {
            id: 0,
            line: f.line,
            col: f.col,
            k: K::Break,
        };
        let bt = self.block(&f.body, if unit { None } else { Some(&f.ret) }, !unit, &at);
        if !unit && !compat(&bt, &f.ret) {
            let msg = format!(
                "fn {} returns {} but its body has type {}",
                f.name, f.ret, bt
            );
            let last = f.body.last().unwrap();
            self.err(Diag::err("E0201", last.line, last.col, msg));
        }
        self.pop_scope(bt != Ty::Never);
    }
}

fn compute_writes(p: &Program) -> HashMap<String, Vec<bool>> {
    let mut w: HashMap<String, Vec<bool>> = p
        .fns
        .iter()
        .map(|f| (f.name.clone(), vec![false; f.params.len()]))
        .collect();
    let rets: HashMap<&str, bool> = p
        .fns
        .iter()
        .map(|f| (f.name.as_str(), f.ret.is_linear()))
        .collect();
    loop {
        let mut changed = false;
        for f in &p.fns {
            for (i, (pn, _)) in f.params.iter().enumerate() {
                if w[&f.name][i] {
                    continue;
                }
                let is_p = |e: &Expr| var_name(e) == Some(pn.as_str());
                let mut hit = false;
                for b in &f.body {
                    b.visit(&mut |e| {
                        hit |= match &e.k {
                            K::Prim(
                                Prim::Put | Prim::Xadd | Prim::Cas | Prim::Free | Prim::Freeze,
                                a,
                                _,
                            ) => a.first().map_or(false, is_p),
                            K::Prim(Prim::Send, a, _) => a.get(1).map_or(false, is_p),
                            K::Call(g, a) => a.iter().enumerate().any(|(j, x)| {
                                is_p(x)
                                    && (w.get(g).and_then(|v| v.get(j)).copied().unwrap_or(false)
                                        || rets.get(g.as_str()).copied().unwrap_or(false))
                            }),
                            K::Spawn(_, a) | K::Go(_, _, a) => a.iter().any(is_p),
                            K::Let { init, .. } => is_p(init),
                            K::Set(_, v) => is_p(v),
                            _ => false,
                        };
                    });
                }
                if hit {
                    w.get_mut(&f.name).unwrap()[i] = true;
                    changed = true;
                }
            }
        }
        if !changed {
            return w;
        }
    }
}

fn struct_cycles(p: &Program, diags: &mut Vec<Diag>) {
    fn visit(n: &str, p: &Program, stack: &mut Vec<String>) -> bool {
        if stack.iter().any(|s| s == n) {
            return true;
        }
        let Some(sd) = p.structs.iter().find(|s| s.name == n) else {
            return false;
        };
        stack.push(n.to_string());
        for (_, t) in &sd.fields {
            if let Ty::Struct(m) = t {
                if visit(m, p, stack) {
                    return true;
                }
            }
        }
        stack.pop();
        false
    }
    for s in &p.structs {
        if visit(&s.name, p, &mut vec![]) {
            diags.push(Diag::err(
                "E0211",
                s.line,
                s.col,
                format!("struct {} contains itself", s.name),
            ));
        }
    }
}

pub fn check(p: &Program) -> (Info, Vec<Diag>) {
    let mut ck = Ck {
        fns: HashMap::new(),
        structs: HashMap::new(),
        writes: compute_writes(p),
        diags: vec![],
        types: HashMap::new(),
        scopes: vec![],
        loop_depth: 0,
        effs: None,
        reported_effs: vec![],
        ctx: String::new(),
        ret_ty: None,
        in_contract: false,
        pfor: None,
    };
    for s in &p.structs {
        if ck.structs.insert(s.name.clone(), s).is_some() {
            ck.err(Diag::err(
                "E0105",
                s.line,
                s.col,
                format!("duplicate struct {}", s.name),
            ));
        }
    }
    for f in &p.fns {
        if ck.fns.insert(f.name.clone(), f).is_some() {
            ck.err(Diag::err(
                "E0105",
                f.line,
                f.col,
                format!("duplicate fn {}", f.name),
            ));
        }
    }
    let mut seen = std::collections::HashSet::new();
    for (n, l, c) in p
        .tests
        .iter()
        .map(|t| (&t.name, t.line, t.col))
        .chain(p.props.iter().map(|t| (&t.name, t.line, t.col)))
    {
        if !seen.insert(n.clone()) {
            ck.err(Diag::err("E0105", l, c, format!("duplicate test {}", n)));
        }
    }
    let mut cyc = vec![];
    struct_cycles(p, &mut cyc);
    ck.diags.extend(cyc);
    for s in &p.structs {
        let mut names = std::collections::HashSet::new();
        for (n, t) in &s.fields {
            if !names.insert(n) {
                ck.err(Diag::err(
                    "E0105",
                    s.line,
                    s.col,
                    format!("duplicate field {}", n),
                ));
            }
            if ck.ty_ok(t, s.line, s.col) && !scalar(t) {
                ck.err(Diag::err(
                    "E0212",
                    s.line,
                    s.col,
                    format!(
                        "struct fields must be numbers, bools or structs, found {}",
                        t
                    ),
                ));
            }
        }
    }
    if let Some(m) = p.fns.iter().find(|f| f.name == "main") {
        if !m.params.is_empty() || !matches!(m.ret, Ty::I64 | Ty::Unit) {
            ck.err(
                Diag::err("E0210", m.line, m.col, "invalid main signature")
                    .fix("(fn main () i64 ...)"),
            );
        }
    }
    for f in &p.fns {
        ck.check_fn(f);
    }
    for t in &p.tests {
        ck.reset(format!("test {}", t.name), None, None);
        let at = Expr {
            id: 0,
            line: t.line,
            col: t.col,
            k: K::Break,
        };
        ck.block(&t.body, None, false, &at);
    }
    for t in &p.props {
        ck.reset(format!("prop {}", t.name), None, None);
        ck.scopes.push(vec![]);
        for (n, ty, _, _) in &t.vars {
            ck.declare(n, ty.clone(), false, t.line, t.col);
        }
        let at = Expr {
            id: 0,
            line: t.line,
            col: t.col,
            k: K::Break,
        };
        let bt = ck.block(&t.body, Some(&Ty::Bool), true, &at);
        if !compat(&bt, &Ty::Bool) {
            ck.err(Diag::err(
                "E0201",
                t.line,
                t.col,
                format!("prop body must end in a bool, found {}", bt),
            ));
        }
    }
    (Info { types: ck.types }, ck.diags)
}
