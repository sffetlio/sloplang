//! Typed abstract syntax tree.

use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Ty {
    I8,
    I16,
    I32,
    I64,
    U8,
    U16,
    U32,
    U64,
    F32,
    F64,
    Bool,
    Unit,
    Str,
    /// Type of expressions that never produce a value (ret, break, continue).
    Never,
    Buf(Box<Ty>),
    Ro(Box<Ty>),
    Chan(Box<Ty>),
    Task(Box<Ty>),
    Group(Box<Ty>),
    Struct(String),
}

impl Ty {
    pub fn is_int(&self) -> bool {
        matches!(
            self,
            Ty::I8 | Ty::I16 | Ty::I32 | Ty::I64 | Ty::U8 | Ty::U16 | Ty::U32 | Ty::U64
        )
    }
    pub fn is_signed(&self) -> bool {
        matches!(self, Ty::I8 | Ty::I16 | Ty::I32 | Ty::I64)
    }
    pub fn is_float(&self) -> bool {
        matches!(self, Ty::F32 | Ty::F64)
    }
    pub fn is_num(&self) -> bool {
        self.is_int() || self.is_float()
    }
    pub fn bits(&self) -> u32 {
        match self {
            Ty::I8 | Ty::U8 => 8,
            Ty::I16 | Ty::U16 => 16,
            Ty::I32 | Ty::U32 | Ty::F32 => 32,
            _ => 64,
        }
    }
    /// Inclusive range of an integer type.
    pub fn range(&self) -> (i128, i128) {
        let b = self.bits();
        if self.is_signed() {
            (-(1i128 << (b - 1)), (1i128 << (b - 1)) - 1)
        } else {
            (0, (1i128 << b) - 1)
        }
    }
    /// Values of these types are moved, never copied.
    pub fn is_linear(&self) -> bool {
        matches!(self, Ty::Buf(_) | Ty::Task(_) | Ty::Group(_))
    }
    /// Linear values that must be consumed exactly once (joined).
    pub fn must_consume(&self) -> bool {
        matches!(self, Ty::Task(_) | Ty::Group(_))
    }
    pub fn from_name(s: &str) -> Option<Ty> {
        Some(match s {
            "i8" => Ty::I8,
            "i16" => Ty::I16,
            "i32" => Ty::I32,
            "i64" => Ty::I64,
            "u8" => Ty::U8,
            "u16" => Ty::U16,
            "u32" => Ty::U32,
            "u64" => Ty::U64,
            "f32" => Ty::F32,
            "f64" => Ty::F64,
            "bool" => Ty::Bool,
            "unit" => Ty::Unit,
            "str" => Ty::Str,
            _ => return None,
        })
    }
}

impl fmt::Display for Ty {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Ty::I8 => write!(f, "i8"),
            Ty::I16 => write!(f, "i16"),
            Ty::I32 => write!(f, "i32"),
            Ty::I64 => write!(f, "i64"),
            Ty::U8 => write!(f, "u8"),
            Ty::U16 => write!(f, "u16"),
            Ty::U32 => write!(f, "u32"),
            Ty::U64 => write!(f, "u64"),
            Ty::F32 => write!(f, "f32"),
            Ty::F64 => write!(f, "f64"),
            Ty::Bool => write!(f, "bool"),
            Ty::Unit => write!(f, "unit"),
            Ty::Str => write!(f, "str"),
            Ty::Never => write!(f, "never"),
            Ty::Buf(t) => write!(f, "(buf {})", t),
            Ty::Ro(t) => write!(f, "(ro {})", t),
            Ty::Chan(t) => write!(f, "(chan {})", t),
            Ty::Task(t) => write!(f, "(task {})", t),
            Ty::Group(t) => write!(f, "(group {})", t),
            Ty::Struct(s) => write!(f, "{}", s),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Eff {
    Io,
    Mem,
    Conc,
}

impl Eff {
    pub fn name(self) -> &'static str {
        match self {
            Eff::Io => "io",
            Eff::Mem => "mem",
            Eff::Conc => "conc",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Op {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    AddW,
    SubW,
    MulW,
    BAnd,
    BOr,
    BXor,
    Shl,
    Shr,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

impl Op {
    pub fn from_sym(s: &str) -> Option<Op> {
        Some(match s {
            "+" => Op::Add,
            "-" => Op::Sub,
            "*" => Op::Mul,
            "/" => Op::Div,
            "%" => Op::Rem,
            "+%" => Op::AddW,
            "-%" => Op::SubW,
            "*%" => Op::MulW,
            "&" => Op::BAnd,
            "|" => Op::BOr,
            "^" => Op::BXor,
            "<<" => Op::Shl,
            ">>" => Op::Shr,
            "==" => Op::Eq,
            "!=" => Op::Ne,
            "<" => Op::Lt,
            "<=" => Op::Le,
            ">" => Op::Gt,
            ">=" => Op::Ge,
            _ => return None,
        })
    }
    pub fn is_cmp(self) -> bool {
        matches!(self, Op::Eq | Op::Ne | Op::Lt | Op::Le | Op::Gt | Op::Ge)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Prim {
    Get,
    Put,
    Len,
    Free,
    Freeze,
    Xadd,
    Cas,
    Join,
    Wait,
    Send,
    Recv,
    Close,
    Print,
    Clock,
    Assert,
}

impl Prim {
    pub fn from_sym(s: &str) -> Option<Prim> {
        Some(match s {
            "get" => Prim::Get,
            "put" => Prim::Put,
            "len" => Prim::Len,
            "free" => Prim::Free,
            "freeze" => Prim::Freeze,
            "xadd" => Prim::Xadd,
            "cas" => Prim::Cas,
            "join" => Prim::Join,
            "wait" => Prim::Wait,
            "send" => Prim::Send,
            "recv" => Prim::Recv,
            "close" => Prim::Close,
            "print" => Prim::Print,
            "clock" => Prim::Clock,
            "assert" => Prim::Assert,
            _ => return None,
        })
    }
}

pub type B = Box<Expr>;

#[derive(Clone, Debug)]
pub struct Expr {
    pub id: usize,
    pub line: u32,
    pub col: u32,
    pub k: K,
}

#[derive(Clone, Debug)]
pub enum K {
    Int(i128, Option<Ty>),
    Float(f64, Option<Ty>),
    Bool(bool),
    Str(String),
    Var(String),
    Let {
        name: String,
        mutable: bool,
        ty: Option<Ty>,
        init: B,
    },
    Set(B, B),
    If(B, B, Option<B>),
    While(B, Vec<Expr>),
    For(String, B, B, Vec<Expr>),
    Pfor(String, B, B, Vec<Expr>),
    Each(String, B, Vec<Expr>),
    Do(Vec<Expr>),
    Ret(Option<B>),
    Break,
    Continue,
    Bin(Op, B, B),
    And(B, B),
    Or(B, B),
    Not(B),
    Neg(B),
    As(Ty, B),
    Mk(String, Vec<Expr>),
    Field(B, String),
    Call(String, Vec<Expr>),
    /// (spawn (f args...)): the inner Call is stored inline.
    Spawn(String, Vec<Expr>),
    /// (go g (f args...))
    Go(B, String, Vec<Expr>),
    Alloc(Ty, B),
    Chan(Ty, B),
    Group(Ty),
    /// Prim op; `text` is the canonical source of the form for trap messages.
    Prim(Prim, Vec<Expr>, String),
}

#[derive(Clone, Debug)]
pub struct Contract {
    pub e: Expr,
    pub text: String,
}

#[derive(Clone, Debug)]
pub struct FnDef {
    pub name: String,
    pub params: Vec<(String, Ty)>,
    pub ret: Ty,
    pub effs: Vec<Eff>,
    pub doc: Option<String>,
    pub pre: Vec<Contract>,
    pub post: Vec<Contract>,
    pub body: Vec<Expr>,
    pub line: u32,
    pub col: u32,
}

#[derive(Clone, Debug)]
pub struct StructDef {
    pub name: String,
    pub fields: Vec<(String, Ty)>,
    pub line: u32,
    pub col: u32,
}

#[derive(Clone, Debug)]
pub struct TestDef {
    pub name: String,
    pub body: Vec<Expr>,
    pub line: u32,
    pub col: u32,
}

#[derive(Clone, Debug)]
pub struct PropDef {
    pub name: String,
    pub vars: Vec<(String, Ty, i128, i128)>,
    pub body: Vec<Expr>,
    pub line: u32,
    pub col: u32,
}

#[derive(Default, Debug)]
pub struct Program {
    pub fns: Vec<FnDef>,
    pub structs: Vec<StructDef>,
    pub tests: Vec<TestDef>,
    pub props: Vec<PropDef>,
}

impl Expr {
    /// Direct sub-expressions, in evaluation order.
    pub fn children(&self) -> Vec<&Expr> {
        let mut v: Vec<&Expr> = vec![];
        match &self.k {
            K::Int(..)
            | K::Float(..)
            | K::Bool(_)
            | K::Str(_)
            | K::Var(_)
            | K::Break
            | K::Continue
            | K::Group(_) => {}
            K::Let { init, .. } => v.push(init),
            K::Set(a, b) | K::Bin(_, a, b) | K::And(a, b) | K::Or(a, b) => {
                v.push(a);
                v.push(b);
            }
            K::If(c, t, e) => {
                v.push(c);
                v.push(t);
                if let Some(e) = e {
                    v.push(e);
                }
            }
            K::While(c, body) | K::Each(_, c, body) => {
                v.push(c);
                v.extend(body.iter());
            }
            K::For(_, lo, hi, body) | K::Pfor(_, lo, hi, body) => {
                v.push(lo);
                v.push(hi);
                v.extend(body.iter());
            }
            K::Do(body)
            | K::Mk(_, body)
            | K::Call(_, body)
            | K::Spawn(_, body)
            | K::Prim(_, body, _) => v.extend(body.iter()),
            K::Ret(x) => {
                if let Some(x) = x {
                    v.push(x)
                }
            }
            K::Not(a)
            | K::Neg(a)
            | K::As(_, a)
            | K::Field(a, _)
            | K::Alloc(_, a)
            | K::Chan(_, a) => v.push(a),
            K::Go(g, _, args) => {
                v.push(g);
                v.extend(args.iter());
            }
        }
        v
    }

    pub fn visit<'a>(&'a self, f: &mut dyn FnMut(&'a Expr)) {
        f(self);
        for c in self.children() {
            c.visit(f);
        }
    }
}
