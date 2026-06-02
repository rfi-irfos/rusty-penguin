// tjs.rs — TernaryJS, BRICK 1: a tree-walk interpreter for a useful JS subset.
//
// no_std + alloc only. This is NOT a full ECMAScript engine (no prototypes,
// closures-over-mutable-upvalues, objects, exceptions, generators, async, etc.).
// It is a working subset: numbers/strings/bools/null, arrays, var/let/const,
// binary+unary ops, if/else, while, C-style for, blocks, function decls + calls,
// return, and console.log(...) into a captured buffer. Sandboxed via a step cap.
//
// TERNARY ANGLE: all ordering/equality comparisons route through ONE balanced-
// ternary comparator `cmp3` returning {-1,0,+1}; <,<=,>,>=,==,!= are derived
// from that single value. See `ternary_bench()` for an HONEST op-count measure
// of unified-3way vs naive-binary dispatch.

use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use core::cell::Cell;

// ---------------------------------------------------------------------------
// Lexer
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Num(f64),
    Str(String),
    Ident(String),
    Kw(Kw),
    // operators / punctuation
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Assign,
    EqEq,
    EqEqEq,
    NotEq,
    NotEqEq,
    Lt,
    Le,
    Gt,
    Ge,
    AndAnd,
    OrOr,
    Bang,
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBrack,
    RBrack,
    Comma,
    Semi,
    Dot,
    Eof,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Kw {
    Var,
    Let,
    Const,
    If,
    Else,
    While,
    For,
    Function,
    Return,
    True,
    False,
    Null,
}

fn kw_of(s: &str) -> Option<Kw> {
    Some(match s {
        "var" => Kw::Var,
        "let" => Kw::Let,
        "const" => Kw::Const,
        "if" => Kw::If,
        "else" => Kw::Else,
        "while" => Kw::While,
        "for" => Kw::For,
        "function" => Kw::Function,
        "return" => Kw::Return,
        "true" => Kw::True,
        "false" => Kw::False,
        "null" => Kw::Null,
        _ => return None,
    })
}

struct Lexer<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> Lexer<'a> {
    fn new(s: &'a str) -> Self {
        Lexer { b: s.as_bytes(), i: 0 }
    }
    fn peek(&self) -> u8 {
        if self.i < self.b.len() {
            self.b[self.i]
        } else {
            0
        }
    }
    fn peek2(&self) -> u8 {
        if self.i + 1 < self.b.len() {
            self.b[self.i + 1]
        } else {
            0
        }
    }
    fn bump(&mut self) -> u8 {
        let c = self.peek();
        self.i += 1;
        c
    }

    fn skip_trivia(&mut self) -> Result<(), String> {
        loop {
            let c = self.peek();
            if c == b' ' || c == b'\t' || c == b'\r' || c == b'\n' {
                self.i += 1;
            } else if c == b'/' && self.peek2() == b'/' {
                while self.peek() != b'\n' && self.peek() != 0 {
                    self.i += 1;
                }
            } else if c == b'/' && self.peek2() == b'*' {
                self.i += 2;
                loop {
                    if self.peek() == 0 {
                        return Err("tjs: unterminated block comment".into());
                    }
                    if self.peek() == b'*' && self.peek2() == b'/' {
                        self.i += 2;
                        break;
                    }
                    self.i += 1;
                }
            } else {
                return Ok(());
            }
        }
    }

    fn tokens(&mut self) -> Result<Vec<Tok>, String> {
        let mut out = Vec::new();
        loop {
            self.skip_trivia()?;
            let c = self.peek();
            if c == 0 {
                out.push(Tok::Eof);
                return Ok(out);
            }
            // number (incl. float, leading dot)
            if c.is_ascii_digit() || (c == b'.' && self.peek2().is_ascii_digit()) {
                out.push(self.lex_number()?);
                continue;
            }
            // string
            if c == b'\'' || c == b'"' {
                out.push(self.lex_string(c)?);
                continue;
            }
            // identifier / keyword
            if c == b'_' || c == b'$' || c.is_ascii_alphabetic() {
                let start = self.i;
                while {
                    let d = self.peek();
                    d == b'_' || d == b'$' || d.is_ascii_alphanumeric()
                } {
                    self.i += 1;
                }
                let s = core::str::from_utf8(&self.b[start..self.i])
                    .map_err(|_| "tjs: bad utf8 in identifier".to_string())?;
                out.push(match kw_of(s) {
                    Some(k) => Tok::Kw(k),
                    None => Tok::Ident(s.to_string()),
                });
                continue;
            }
            // operators / punctuation
            out.push(self.lex_op()?);
        }
    }

    fn lex_number(&mut self) -> Result<Tok, String> {
        let start = self.i;
        while self.peek().is_ascii_digit() {
            self.i += 1;
        }
        if self.peek() == b'.' {
            self.i += 1;
            while self.peek().is_ascii_digit() {
                self.i += 1;
            }
        }
        // optional exponent
        if self.peek() == b'e' || self.peek() == b'E' {
            self.i += 1;
            if self.peek() == b'+' || self.peek() == b'-' {
                self.i += 1;
            }
            while self.peek().is_ascii_digit() {
                self.i += 1;
            }
        }
        let s = core::str::from_utf8(&self.b[start..self.i]).unwrap_or("");
        let v: f64 = s.parse().map_err(|_| format!("tjs: bad number '{}'", s))?;
        Ok(Tok::Num(v))
    }

    fn lex_string(&mut self, q: u8) -> Result<Tok, String> {
        self.i += 1; // opening quote
        let mut s = String::new();
        loop {
            let c = self.peek();
            if c == 0 {
                return Err("tjs: unterminated string".into());
            }
            if c == q {
                self.i += 1;
                return Ok(Tok::Str(s));
            }
            if c == b'\\' {
                self.i += 1;
                let e = self.bump();
                match e {
                    b'n' => s.push('\n'),
                    b't' => s.push('\t'),
                    b'r' => s.push('\r'),
                    b'\\' => s.push('\\'),
                    b'\'' => s.push('\''),
                    b'"' => s.push('"'),
                    b'0' => s.push('\0'),
                    other => s.push(other as char),
                }
                continue;
            }
            s.push(c as char);
            self.i += 1;
        }
    }

    fn lex_op(&mut self) -> Result<Tok, String> {
        let c = self.bump();
        Ok(match c {
            b'+' => Tok::Plus,
            b'-' => Tok::Minus,
            b'*' => Tok::Star,
            b'/' => Tok::Slash,
            b'%' => Tok::Percent,
            b'(' => Tok::LParen,
            b')' => Tok::RParen,
            b'{' => Tok::LBrace,
            b'}' => Tok::RBrace,
            b'[' => Tok::LBrack,
            b']' => Tok::RBrack,
            b',' => Tok::Comma,
            b';' => Tok::Semi,
            b'.' => Tok::Dot,
            b'=' => {
                if self.peek() == b'=' {
                    self.i += 1;
                    if self.peek() == b'=' {
                        self.i += 1;
                        Tok::EqEqEq
                    } else {
                        Tok::EqEq
                    }
                } else {
                    Tok::Assign
                }
            }
            b'!' => {
                if self.peek() == b'=' {
                    self.i += 1;
                    if self.peek() == b'=' {
                        self.i += 1;
                        Tok::NotEqEq
                    } else {
                        Tok::NotEq
                    }
                } else {
                    Tok::Bang
                }
            }
            b'<' => {
                if self.peek() == b'=' {
                    self.i += 1;
                    Tok::Le
                } else {
                    Tok::Lt
                }
            }
            b'>' => {
                if self.peek() == b'=' {
                    self.i += 1;
                    Tok::Ge
                } else {
                    Tok::Gt
                }
            }
            b'&' => {
                if self.peek() == b'&' {
                    self.i += 1;
                    Tok::AndAnd
                } else {
                    return Err("tjs: lone '&' (bitwise not supported)".into());
                }
            }
            b'|' => {
                if self.peek() == b'|' {
                    self.i += 1;
                    Tok::OrOr
                } else {
                    return Err("tjs: lone '|' (bitwise not supported)".into());
                }
            }
            other => return Err(format!("tjs: unexpected char '{}'", other as char)),
        })
    }
}

// ---------------------------------------------------------------------------
// AST
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
enum Expr {
    Num(f64),
    Str(String),
    Bool(bool),
    Null,
    Ident(String),
    Array(Vec<Expr>),
    Index(Box<Expr>, Box<Expr>),
    Unary(UnOp, Box<Expr>),
    Bin(BinOp, Box<Expr>, Box<Expr>),
    Logic(LogicOp, Box<Expr>, Box<Expr>),
    Assign(Box<Expr>, Box<Expr>), // target (Ident or Index), value
    Call(Box<Expr>, Vec<Expr>),
    Member(Box<Expr>, String), // a.b  (used for console.log + array.length)
}

#[derive(Clone, Copy, Debug)]
enum UnOp {
    Neg,
    Not,
}

#[derive(Clone, Copy, Debug)]
enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    // comparisons (all routed through cmp3)
    Lt,
    Le,
    Gt,
    Ge,
    EqLoose,
    NeLoose,
    EqStrict,
    NeStrict,
}

#[derive(Clone, Copy, Debug)]
enum LogicOp {
    And,
    Or,
}

#[derive(Clone, Debug)]
enum Stmt {
    Expr(Expr),
    Decl(String, Option<Expr>),
    Block(Vec<Stmt>),
    If(Expr, Box<Stmt>, Option<Box<Stmt>>),
    While(Expr, Box<Stmt>),
    For(Option<Box<Stmt>>, Option<Expr>, Option<Box<Stmt>>, Box<Stmt>),
    Func(String, Vec<String>, Vec<Stmt>),
    Return(Option<Expr>),
}

// ---------------------------------------------------------------------------
// Parser (Pratt / precedence-climbing for expressions)
// ---------------------------------------------------------------------------

struct Parser {
    t: Vec<Tok>,
    i: usize,
}

impl Parser {
    fn new(t: Vec<Tok>) -> Self {
        Parser { t, i: 0 }
    }
    fn peek(&self) -> &Tok {
        &self.t[self.i]
    }
    fn bump(&mut self) -> Tok {
        let t = self.t[self.i].clone();
        if self.i + 1 < self.t.len() {
            self.i += 1;
        }
        t
    }
    fn eat(&mut self, t: &Tok) -> Result<(), String> {
        if self.peek() == t {
            self.bump();
            Ok(())
        } else {
            Err(format!("tjs: expected {:?}, found {:?}", t, self.peek()))
        }
    }

    fn program(&mut self) -> Result<Vec<Stmt>, String> {
        let mut v = Vec::new();
        while *self.peek() != Tok::Eof {
            v.push(self.stmt()?);
        }
        Ok(v)
    }

    fn stmt(&mut self) -> Result<Stmt, String> {
        match self.peek().clone() {
            Tok::Kw(Kw::Var) | Tok::Kw(Kw::Let) | Tok::Kw(Kw::Const) => {
                self.bump();
                let name = self.ident()?;
                let init = if *self.peek() == Tok::Assign {
                    self.bump();
                    Some(self.expr()?)
                } else {
                    None
                };
                self.semi_opt();
                Ok(Stmt::Decl(name, init))
            }
            Tok::LBrace => Ok(Stmt::Block(self.block()?)),
            Tok::Kw(Kw::If) => self.if_stmt(),
            Tok::Kw(Kw::While) => {
                self.bump();
                self.eat(&Tok::LParen)?;
                let c = self.expr()?;
                self.eat(&Tok::RParen)?;
                let body = self.stmt()?;
                Ok(Stmt::While(c, Box::new(body)))
            }
            Tok::Kw(Kw::For) => self.for_stmt(),
            Tok::Kw(Kw::Function) => {
                self.bump();
                let name = self.ident()?;
                let params = self.params()?;
                let body = self.block()?;
                Ok(Stmt::Func(name, params, body))
            }
            Tok::Kw(Kw::Return) => {
                self.bump();
                let e = if *self.peek() == Tok::Semi || *self.peek() == Tok::RBrace {
                    None
                } else {
                    Some(self.expr()?)
                };
                self.semi_opt();
                Ok(Stmt::Return(e))
            }
            _ => {
                let e = self.expr()?;
                self.semi_opt();
                Ok(Stmt::Expr(e))
            }
        }
    }

    fn if_stmt(&mut self) -> Result<Stmt, String> {
        self.bump(); // if
        self.eat(&Tok::LParen)?;
        let c = self.expr()?;
        self.eat(&Tok::RParen)?;
        let then = Box::new(self.stmt()?);
        let els = if *self.peek() == Tok::Kw(Kw::Else) {
            self.bump();
            Some(Box::new(self.stmt()?))
        } else {
            None
        };
        Ok(Stmt::If(c, then, els))
    }

    fn for_stmt(&mut self) -> Result<Stmt, String> {
        self.bump(); // for
        self.eat(&Tok::LParen)?;
        // init
        let init = if *self.peek() == Tok::Semi {
            self.bump();
            None
        } else {
            let s = self.simple_for_init()?;
            self.eat(&Tok::Semi)?;
            Some(Box::new(s))
        };
        // cond
        let cond = if *self.peek() == Tok::Semi {
            None
        } else {
            Some(self.expr()?)
        };
        self.eat(&Tok::Semi)?;
        // step
        let step = if *self.peek() == Tok::RParen {
            None
        } else {
            Some(Box::new(Stmt::Expr(self.expr()?)))
        };
        self.eat(&Tok::RParen)?;
        let body = Box::new(self.stmt()?);
        Ok(Stmt::For(init, cond, step, body))
    }

    // for-init allows `let i=0` or a bare expression, no trailing semi consumed
    fn simple_for_init(&mut self) -> Result<Stmt, String> {
        match self.peek().clone() {
            Tok::Kw(Kw::Var) | Tok::Kw(Kw::Let) | Tok::Kw(Kw::Const) => {
                self.bump();
                let name = self.ident()?;
                let init = if *self.peek() == Tok::Assign {
                    self.bump();
                    Some(self.expr()?)
                } else {
                    None
                };
                Ok(Stmt::Decl(name, init))
            }
            _ => Ok(Stmt::Expr(self.expr()?)),
        }
    }

    fn block(&mut self) -> Result<Vec<Stmt>, String> {
        self.eat(&Tok::LBrace)?;
        let mut v = Vec::new();
        while *self.peek() != Tok::RBrace && *self.peek() != Tok::Eof {
            v.push(self.stmt()?);
        }
        self.eat(&Tok::RBrace)?;
        Ok(v)
    }

    fn params(&mut self) -> Result<Vec<String>, String> {
        self.eat(&Tok::LParen)?;
        let mut v = Vec::new();
        while *self.peek() != Tok::RParen {
            v.push(self.ident()?);
            if *self.peek() == Tok::Comma {
                self.bump();
            } else {
                break;
            }
        }
        self.eat(&Tok::RParen)?;
        Ok(v)
    }

    fn ident(&mut self) -> Result<String, String> {
        match self.bump() {
            Tok::Ident(s) => Ok(s),
            t => Err(format!("tjs: expected identifier, found {:?}", t)),
        }
    }

    fn semi_opt(&mut self) {
        if *self.peek() == Tok::Semi {
            self.bump();
        }
    }

    // ---- expressions ----
    // Entry: assignment has the lowest precedence and is right-assoc.
    fn expr(&mut self) -> Result<Expr, String> {
        let lhs = self.bin_expr(0)?;
        if *self.peek() == Tok::Assign {
            self.bump();
            // only Ident or Index are valid targets
            match &lhs {
                Expr::Ident(_) | Expr::Index(_, _) => {}
                _ => return Err("tjs: invalid assignment target".into()),
            }
            let rhs = self.expr()?;
            return Ok(Expr::Assign(Box::new(lhs), Box::new(rhs)));
        }
        Ok(lhs)
    }

    // precedence-climbing over binary + logical operators
    fn bin_expr(&mut self, min_bp: u8) -> Result<Expr, String> {
        let mut lhs = self.unary()?;
        loop {
            let (lbp, _) = match self.bin_bp() {
                Some(x) => x,
                None => break,
            };
            if lbp < min_bp {
                break;
            }
            let op = self.bump();
            let rhs = self.bin_expr(lbp + 1)?;
            lhs = Self::mk_bin(op, lhs, rhs);
        }
        Ok(lhs)
    }

    // binding power table (left bp, right bp); higher = tighter.
    fn bin_bp(&self) -> Option<(u8, u8)> {
        Some(match self.peek() {
            Tok::OrOr => (1, 1),
            Tok::AndAnd => (2, 2),
            Tok::EqEq | Tok::NotEq | Tok::EqEqEq | Tok::NotEqEq => (3, 3),
            Tok::Lt | Tok::Le | Tok::Gt | Tok::Ge => (4, 4),
            Tok::Plus | Tok::Minus => (5, 5),
            Tok::Star | Tok::Slash | Tok::Percent => (6, 6),
            _ => return None,
        })
    }

    fn mk_bin(op: Tok, l: Expr, r: Expr) -> Expr {
        let (lb, rb) = (Box::new(l), Box::new(r));
        match op {
            Tok::OrOr => Expr::Logic(LogicOp::Or, lb, rb),
            Tok::AndAnd => Expr::Logic(LogicOp::And, lb, rb),
            Tok::Plus => Expr::Bin(BinOp::Add, lb, rb),
            Tok::Minus => Expr::Bin(BinOp::Sub, lb, rb),
            Tok::Star => Expr::Bin(BinOp::Mul, lb, rb),
            Tok::Slash => Expr::Bin(BinOp::Div, lb, rb),
            Tok::Percent => Expr::Bin(BinOp::Mod, lb, rb),
            Tok::Lt => Expr::Bin(BinOp::Lt, lb, rb),
            Tok::Le => Expr::Bin(BinOp::Le, lb, rb),
            Tok::Gt => Expr::Bin(BinOp::Gt, lb, rb),
            Tok::Ge => Expr::Bin(BinOp::Ge, lb, rb),
            Tok::EqEq => Expr::Bin(BinOp::EqLoose, lb, rb),
            Tok::NotEq => Expr::Bin(BinOp::NeLoose, lb, rb),
            Tok::EqEqEq => Expr::Bin(BinOp::EqStrict, lb, rb),
            Tok::NotEqEq => Expr::Bin(BinOp::NeStrict, lb, rb),
            _ => unreachable!(),
        }
    }

    fn unary(&mut self) -> Result<Expr, String> {
        match self.peek() {
            Tok::Minus => {
                self.bump();
                Ok(Expr::Unary(UnOp::Neg, Box::new(self.unary()?)))
            }
            Tok::Bang => {
                self.bump();
                Ok(Expr::Unary(UnOp::Not, Box::new(self.unary()?)))
            }
            _ => self.postfix(),
        }
    }

    // postfix: calls, member access, indexing
    fn postfix(&mut self) -> Result<Expr, String> {
        let mut e = self.primary()?;
        loop {
            match self.peek() {
                Tok::LParen => {
                    self.bump();
                    let mut args = Vec::new();
                    while *self.peek() != Tok::RParen {
                        args.push(self.expr()?);
                        if *self.peek() == Tok::Comma {
                            self.bump();
                        } else {
                            break;
                        }
                    }
                    self.eat(&Tok::RParen)?;
                    e = Expr::Call(Box::new(e), args);
                }
                Tok::Dot => {
                    self.bump();
                    let name = self.ident()?;
                    e = Expr::Member(Box::new(e), name);
                }
                Tok::LBrack => {
                    self.bump();
                    let idx = self.expr()?;
                    self.eat(&Tok::RBrack)?;
                    e = Expr::Index(Box::new(e), Box::new(idx));
                }
                _ => break,
            }
        }
        Ok(e)
    }

    fn primary(&mut self) -> Result<Expr, String> {
        match self.bump() {
            Tok::Num(n) => Ok(Expr::Num(n)),
            Tok::Str(s) => Ok(Expr::Str(s)),
            Tok::Kw(Kw::True) => Ok(Expr::Bool(true)),
            Tok::Kw(Kw::False) => Ok(Expr::Bool(false)),
            Tok::Kw(Kw::Null) => Ok(Expr::Null),
            Tok::Ident(s) => Ok(Expr::Ident(s)),
            Tok::LParen => {
                let e = self.expr()?;
                self.eat(&Tok::RParen)?;
                Ok(e)
            }
            Tok::LBrack => {
                let mut v = Vec::new();
                while *self.peek() != Tok::RBrack {
                    v.push(self.expr()?);
                    if *self.peek() == Tok::Comma {
                        self.bump();
                    } else {
                        break;
                    }
                }
                self.eat(&Tok::RBrack)?;
                Ok(Expr::Array(v))
            }
            t => Err(format!("tjs: unexpected token in expression: {:?}", t)),
        }
    }
}

// ---------------------------------------------------------------------------
// Values + environment
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub enum Value {
    Num(f64),
    Str(String),
    Bool(bool),
    Null,
    Array(Vec<Value>),
    // Func carries its definition opaquely so the public `Value` enum does not
    // leak the private AST `Stmt` type.
    Func(Box<FnDef>),
}

#[derive(Clone)]
pub struct FnDef {
    params: Vec<String>,
    body: Vec<Stmt>,
}

impl Value {
    fn truthy(&self) -> bool {
        match self {
            Value::Num(n) => *n != 0.0 && !n.is_nan(),
            Value::Str(s) => !s.is_empty(),
            Value::Bool(b) => *b,
            Value::Null => false,
            Value::Array(_) => true,
            Value::Func(_) => true,
        }
    }
    fn type_tag(&self) -> u8 {
        match self {
            Value::Num(_) => 0,
            Value::Str(_) => 1,
            Value::Bool(_) => 2,
            Value::Null => 3,
            Value::Array(_) => 4,
            Value::Func(_) => 5,
        }
    }
    fn display(&self) -> String {
        match self {
            Value::Num(n) => fmt_num(*n),
            Value::Str(s) => s.clone(),
            Value::Bool(b) => if *b { "true" } else { "false" }.to_string(),
            Value::Null => "null".to_string(),
            Value::Array(a) => {
                let mut s = String::new();
                for (i, v) in a.iter().enumerate() {
                    if i > 0 {
                        s.push(',');
                    }
                    s.push_str(&v.display());
                }
                s
            }
            Value::Func(_) => "[function]".to_string(),
        }
    }
}

// JS-ish number formatting: integers without trailing ".0".
fn fmt_num(n: f64) -> String {
    if n.is_nan() {
        return "NaN".to_string();
    }
    if n.is_infinite() {
        return if n < 0.0 { "-Infinity" } else { "Infinity" }.to_string();
    }
    if n == libm::trunc(n) && libm::fabs(n) < 1e15 {
        // integral value: print without fractional part
        return format!("{}", n as i64);
    }
    format!("{}", n)
}

// A scope = flat Vec of (name, value). Environment = stack of scopes.
struct Env {
    scopes: Vec<Vec<(String, Value)>>,
}

impl Env {
    fn new() -> Self {
        Env { scopes: vec![Vec::new()] }
    }
    fn push(&mut self) {
        self.scopes.push(Vec::new());
    }
    fn pop(&mut self) {
        self.scopes.pop();
    }
    fn declare(&mut self, name: &str, v: Value) {
        let top = self.scopes.last_mut().unwrap();
        // shadow within same scope = overwrite
        if let Some(slot) = top.iter_mut().find(|(n, _)| n == name) {
            slot.1 = v;
        } else {
            top.push((name.to_string(), v));
        }
    }
    fn get(&self, name: &str) -> Option<Value> {
        for sc in self.scopes.iter().rev() {
            if let Some((_, v)) = sc.iter().find(|(n, _)| n == name) {
                return Some(v.clone());
            }
        }
        None
    }
    fn set(&mut self, name: &str, v: Value) -> Result<(), String> {
        for sc in self.scopes.iter_mut().rev() {
            if let Some(slot) = sc.iter_mut().find(|(n, _)| n == name) {
                slot.1 = v;
                return Ok(());
            }
        }
        Err(format!("tjs: assignment to undeclared '{}'", name))
    }
}

// ---------------------------------------------------------------------------
// Balanced-ternary comparator — the shared 3-way primitive.
// ---------------------------------------------------------------------------

// Global op-counter for the honest ternary benchmark. no_std has no
// thread_local!; a plain static Cell suffices because the interpreter runs in a
// single cooperative context. Not touched by normal eval().
static CMP_OPS: SyncCell = SyncCell(Cell::new(0u64));
struct SyncCell(Cell<u64>);
unsafe impl Sync for SyncCell {}

#[inline]
fn cmp_tick() {
    CMP_OPS.0.set(CMP_OPS.0.get() + 1);
}

/// Balanced-ternary compare of two values → exactly one of {-1, 0, +1}.
/// This single result drives <, <=, >, >=, ==, != (and could drive switch-like
/// dispatch). Numbers compare numerically; strings lexicographically; equal
/// types only. Mixed/uncomparable → error (we do NOT silently coerce; honest).
fn cmp3(a: &Value, b: &Value) -> Result<i8, String> {
    cmp_tick();
    match (a, b) {
        (Value::Num(x), Value::Num(y)) => Ok(if x < y {
            -1
        } else if x > y {
            1
        } else {
            0
        }),
        (Value::Str(x), Value::Str(y)) => Ok(if x < y {
            -1
        } else if x > y {
            1
        } else {
            0
        }),
        (Value::Bool(x), Value::Bool(y)) => Ok((*x as i8) - (*y as i8)),
        (Value::Null, Value::Null) => Ok(0),
        _ => Err("tjs: cannot compare values of different types".into()),
    }
}

// Loose/strict equality. Strict requires same type tag. Loose, for this subset,
// permits Num<->Bool coercion only (kept deliberately small + honest).
fn eq_loose(a: &Value, b: &Value) -> bool {
    if a.type_tag() == b.type_tag() {
        return cmp3(a, b).map(|c| c == 0).unwrap_or(false);
    }
    match (a, b) {
        (Value::Num(x), Value::Bool(y)) | (Value::Bool(y), Value::Num(x)) => {
            *x == (*y as i32 as f64)
        }
        (Value::Null, _) | (_, Value::Null) => false,
        _ => false,
    }
}
fn eq_strict(a: &Value, b: &Value) -> bool {
    a.type_tag() == b.type_tag() && cmp3(a, b).map(|c| c == 0).unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Interpreter
// ---------------------------------------------------------------------------

struct Interp {
    env: Env,
    out: Vec<String>,
    steps: Cell<u64>,
    max_steps: u64,
}

enum Flow {
    Normal,
    Return(Value),
}

impl Interp {
    fn new(max_steps: u64) -> Self {
        Interp {
            env: Env::new(),
            out: Vec::new(),
            steps: Cell::new(0),
            max_steps,
        }
    }

    fn tick(&self) -> Result<(), String> {
        let s = self.steps.get() + 1;
        self.steps.set(s);
        if s > self.max_steps {
            return Err("tjs: step limit exceeded (possible infinite loop)".into());
        }
        Ok(())
    }

    fn run(&mut self, prog: &[Stmt]) -> Result<(), String> {
        // hoist top-level function declarations so calls can precede defs
        for s in prog {
            if let Stmt::Func(name, params, body) = s {
                self.env.declare(
                    name,
                    Value::Func(Box::new(FnDef {
                        params: params.clone(),
                        body: body.clone(),
                    })),
                );
            }
        }
        for s in prog {
            if let Flow::Return(_) = self.exec(s)? {
                break;
            }
        }
        Ok(())
    }

    fn exec(&mut self, s: &Stmt) -> Result<Flow, String> {
        self.tick()?;
        match s {
            Stmt::Expr(e) => {
                self.eval_expr(e)?;
                Ok(Flow::Normal)
            }
            Stmt::Decl(name, init) => {
                let v = match init {
                    Some(e) => self.eval_expr(e)?,
                    None => Value::Null,
                };
                self.env.declare(name, v);
                Ok(Flow::Normal)
            }
            Stmt::Block(b) => {
                self.env.push();
                let r = self.exec_block(b);
                self.env.pop();
                r
            }
            Stmt::If(c, then, els) => {
                if self.eval_expr(c)?.truthy() {
                    self.exec(then)
                } else if let Some(e) = els {
                    self.exec(e)
                } else {
                    Ok(Flow::Normal)
                }
            }
            Stmt::While(c, body) => {
                while self.eval_expr(c)?.truthy() {
                    self.tick()?;
                    if let Flow::Return(v) = self.exec(body)? {
                        return Ok(Flow::Return(v));
                    }
                }
                Ok(Flow::Normal)
            }
            Stmt::For(init, cond, step, body) => {
                self.env.push();
                let r = self.exec_for(init, cond, step, body);
                self.env.pop();
                r
            }
            Stmt::Func(name, params, body) => {
                self.env.declare(
                    name,
                    Value::Func(Box::new(FnDef {
                        params: params.clone(),
                        body: body.clone(),
                    })),
                );
                Ok(Flow::Normal)
            }
            Stmt::Return(e) => {
                let v = match e {
                    Some(x) => self.eval_expr(x)?,
                    None => Value::Null,
                };
                Ok(Flow::Return(v))
            }
        }
    }

    fn exec_for(
        &mut self,
        init: &Option<Box<Stmt>>,
        cond: &Option<Expr>,
        step: &Option<Box<Stmt>>,
        body: &Stmt,
    ) -> Result<Flow, String> {
        if let Some(i) = init {
            self.exec(i)?;
        }
        loop {
            self.tick()?;
            let go = match cond {
                Some(c) => self.eval_expr(c)?.truthy(),
                None => true,
            };
            if !go {
                break;
            }
            if let Flow::Return(v) = self.exec(body)? {
                return Ok(Flow::Return(v));
            }
            if let Some(st) = step {
                self.exec(st)?;
            }
        }
        Ok(Flow::Normal)
    }

    fn exec_block(&mut self, b: &[Stmt]) -> Result<Flow, String> {
        // hoist nested function decls in this block
        for s in b {
            if let Stmt::Func(name, params, body) = s {
                self.env.declare(
                    name,
                    Value::Func(Box::new(FnDef {
                        params: params.clone(),
                        body: body.clone(),
                    })),
                );
            }
        }
        for s in b {
            if let Flow::Return(v) = self.exec(s)? {
                return Ok(Flow::Return(v));
            }
        }
        Ok(Flow::Normal)
    }

    fn eval_expr(&mut self, e: &Expr) -> Result<Value, String> {
        self.tick()?;
        match e {
            Expr::Num(n) => Ok(Value::Num(*n)),
            Expr::Str(s) => Ok(Value::Str(s.clone())),
            Expr::Bool(b) => Ok(Value::Bool(*b)),
            Expr::Null => Ok(Value::Null),
            Expr::Ident(name) => self
                .env
                .get(name)
                .ok_or_else(|| format!("tjs: undefined variable '{}'", name)),
            Expr::Array(items) => {
                let mut v = Vec::with_capacity(items.len());
                for it in items {
                    v.push(self.eval_expr(it)?);
                }
                Ok(Value::Array(v))
            }
            Expr::Index(base, idx) => {
                let b = self.eval_expr(base)?;
                let i = self.eval_expr(idx)?;
                match (b, i) {
                    (Value::Array(a), Value::Num(n)) => {
                        let k = n as i64;
                        if k < 0 || k as usize >= a.len() {
                            Ok(Value::Null)
                        } else {
                            Ok(a[k as usize].clone())
                        }
                    }
                    (Value::Str(s), Value::Num(n)) => {
                        let k = n as i64;
                        match s.chars().nth(k as usize) {
                            Some(c) => Ok(Value::Str(c.to_string())),
                            None => Ok(Value::Null),
                        }
                    }
                    _ => Err("tjs: invalid index operation".into()),
                }
            }
            Expr::Member(base, name) => {
                let b = self.eval_expr(base)?;
                match (&b, name.as_str()) {
                    (Value::Array(a), "length") => Ok(Value::Num(a.len() as f64)),
                    (Value::Str(s), "length") => Ok(Value::Num(s.chars().count() as f64)),
                    // console.log handled at call-site; bare member of console errors
                    _ => Err(format!("tjs: no member '{}' on value", name)),
                }
            }
            Expr::Unary(op, x) => {
                let v = self.eval_expr(x)?;
                match op {
                    UnOp::Neg => match v {
                        Value::Num(n) => Ok(Value::Num(-n)),
                        _ => Err("tjs: unary '-' on non-number".into()),
                    },
                    UnOp::Not => Ok(Value::Bool(!v.truthy())),
                }
            }
            Expr::Logic(op, l, r) => {
                let lv = self.eval_expr(l)?;
                match op {
                    LogicOp::And => {
                        if lv.truthy() {
                            self.eval_expr(r)
                        } else {
                            Ok(lv)
                        }
                    }
                    LogicOp::Or => {
                        if lv.truthy() {
                            Ok(lv)
                        } else {
                            self.eval_expr(r)
                        }
                    }
                }
            }
            Expr::Bin(op, l, r) => {
                let lv = self.eval_expr(l)?;
                let rv = self.eval_expr(r)?;
                self.binop(*op, lv, rv)
            }
            Expr::Assign(target, val) => {
                let v = self.eval_expr(val)?;
                match target.as_ref() {
                    Expr::Ident(name) => {
                        self.env.set(name, v.clone())?;
                        Ok(v)
                    }
                    Expr::Index(base, idx) => {
                        // only Ident-rooted arrays are mutable here
                        let iname = match base.as_ref() {
                            Expr::Ident(n) => n.clone(),
                            _ => return Err("tjs: unsupported assignment target".into()),
                        };
                        let i = self.eval_expr(idx)?;
                        let k = match i {
                            Value::Num(n) => n as i64,
                            _ => return Err("tjs: array index must be a number".into()),
                        };
                        let mut arr = match self.env.get(&iname) {
                            Some(Value::Array(a)) => a,
                            _ => return Err("tjs: index-assign to non-array".into()),
                        };
                        if k < 0 {
                            return Err("tjs: negative array index".into());
                        }
                        let k = k as usize;
                        if k >= arr.len() {
                            arr.resize(k + 1, Value::Null);
                        }
                        arr[k] = v.clone();
                        self.env.set(&iname, Value::Array(arr))?;
                        Ok(v)
                    }
                    _ => Err("tjs: invalid assignment target".into()),
                }
            }
            Expr::Call(callee, args) => self.eval_call(callee, args),
        }
    }

    fn eval_call(&mut self, callee: &Expr, args: &[Expr]) -> Result<Value, String> {
        // console.log(...) special form
        if let Expr::Member(obj, m) = callee {
            if let Expr::Ident(o) = obj.as_ref() {
                if o == "console" && (m == "log" || m == "error" || m == "warn") {
                    let mut parts = Vec::with_capacity(args.len());
                    for a in args {
                        parts.push(self.eval_expr(a)?.display());
                    }
                    self.out.push(parts.join(" "));
                    return Ok(Value::Null);
                }
            }
        }
        // user function call
        let f = self.eval_expr(callee)?;
        let (params, body) = match f {
            Value::Func(d) => (d.params, d.body),
            _ => return Err("tjs: attempt to call a non-function".into()),
        };
        let mut argv = Vec::with_capacity(args.len());
        for a in args {
            argv.push(self.eval_expr(a)?);
        }
        // new call frame: a fresh scope on top (no closures — functions see
        // globals + their own params; honest limitation of BRICK 1).
        self.env.push();
        for (i, p) in params.iter().enumerate() {
            let v = argv.get(i).cloned().unwrap_or(Value::Null);
            self.env.declare(p, v);
        }
        let r = self.exec_block(&body);
        self.env.pop();
        match r? {
            Flow::Return(v) => Ok(v),
            Flow::Normal => Ok(Value::Null),
        }
    }

    fn binop(&self, op: BinOp, l: Value, r: Value) -> Result<Value, String> {
        match op {
            BinOp::Add => match (&l, &r) {
                (Value::Num(a), Value::Num(b)) => Ok(Value::Num(a + b)),
                // string concat if either side is a string (JS-ish)
                (Value::Str(_), _) | (_, Value::Str(_)) => {
                    Ok(Value::Str(format!("{}{}", l.display(), r.display())))
                }
                _ => Err("tjs: '+' on incompatible types".into()),
            },
            BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Mod => {
                let (a, b) = match (&l, &r) {
                    (Value::Num(a), Value::Num(b)) => (*a, *b),
                    _ => return Err("tjs: arithmetic on non-numbers".into()),
                };
                Ok(Value::Num(match op {
                    BinOp::Sub => a - b,
                    BinOp::Mul => a * b,
                    BinOp::Div => a / b,
                    BinOp::Mod => a - b * libm::trunc(a / b),
                    _ => unreachable!(),
                }))
            }
            // ----- ALL comparisons derive from the single cmp3 result -----
            BinOp::Lt => Ok(Value::Bool(cmp3(&l, &r)? < 0)),
            BinOp::Le => Ok(Value::Bool(cmp3(&l, &r)? <= 0)),
            BinOp::Gt => Ok(Value::Bool(cmp3(&l, &r)? > 0)),
            BinOp::Ge => Ok(Value::Bool(cmp3(&l, &r)? >= 0)),
            BinOp::EqLoose => Ok(Value::Bool(eq_loose(&l, &r))),
            BinOp::NeLoose => Ok(Value::Bool(!eq_loose(&l, &r))),
            BinOp::EqStrict => Ok(Value::Bool(eq_strict(&l, &r))),
            BinOp::NeStrict => Ok(Value::Bool(!eq_strict(&l, &r))),
        }
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

const DEFAULT_MAX_STEPS: u64 = 5_000_000;

/// Evaluate a TernaryJS source string. Returns captured console.log output
/// (newline-joined) on success, or an error string. Fully sandboxed: no real
/// I/O, bounded by a step counter.
pub fn eval(src: &str) -> Result<String, String> {
    let toks = Lexer::new(src).tokens()?;
    let prog = Parser::new(toks).program()?;
    let mut interp = Interp::new(DEFAULT_MAX_STEPS);
    interp.run(&prog)?;
    Ok(interp.out.join("\n"))
}

// ---------------------------------------------------------------------------
// Self-test
// ---------------------------------------------------------------------------

/// Run a battery of programs and assert on their captured output.
/// Returns true iff every case passes. Pure, allocation-only, no I/O.
pub fn self_test() -> bool {
    let cases: &[(&str, &str)] = &[
        // arithmetic + precedence
        ("console.log(1 + 2 * 3 - 4 / 2);", "5"),
        ("console.log((1 + 2) * 3);", "9"),
        ("console.log(7 % 3);", "1"),
        // floats
        ("console.log(0.5 + 0.25);", "0.75"),
        // string concat
        ("console.log('foo' + 'bar');", "foobar"),
        ("console.log('n=' + (2 + 3));", "n=5"),
        // booleans / comparisons (all via cmp3)
        ("console.log(3 < 5, 5 <= 5, 9 > 2, 2 >= 3);", "true true true false"),
        ("console.log(2 == 2, 2 != 3, 2 === 2, 2 !== 2);", "true true true false"),
        // unary + logical
        ("console.log(!false, -(-3), true && false, true || false);", "true 3 false true"),
        // if/else
        (
            "var x = 7; if (x > 5) { console.log('big'); } else { console.log('small'); }",
            "big",
        ),
        // while loop counter
        (
            "var i = 0; var s = 0; while (i < 5) { s = s + i; i = i + 1; } console.log(s);",
            "10",
        ),
        // C-style for loop
        (
            "var t = 0; for (var k = 1; k <= 4; k = k + 1) { t = t + k; } console.log(t);",
            "10",
        ),
        // array literal + index + length + sum loop
        (
            "var a = [3, 1, 4, 1, 5, 9]; var sum = 0; for (var i = 0; i < a.length; i = i + 1) { sum = sum + a[i]; } console.log(sum);",
            "23",
        ),
        // array element assignment
        (
            "var a = [0, 0, 0]; a[1] = 42; console.log(a[0], a[1], a[2]);",
            "0 42 0",
        ),
        // function decl + call, used before definition (hoisting)
        (
            "console.log(sq(6)); function sq(n) { return n * n; }",
            "36",
        ),
        // recursion + iterative fib in one program
        (
            "function fib(n) { if (n < 2) { return n; } return fib(n - 1) + fib(n - 2); } console.log(fib(10));",
            "55",
        ),
        (
            "function fibLoop(n) { var a = 0; var b = 1; for (var i = 0; i < n; i = i + 1) { var t = a + b; a = b; b = t; } return a; } console.log(fibLoop(10));",
            "55",
        ),
    ];

    for (src, want) in cases {
        match eval(src) {
            Ok(got) => {
                if got != *want {
                    return false;
                }
            }
            Err(_) => return false,
        }
    }

    // step-limit sandbox must fire on a true infinite loop
    match eval("while (true) { }") {
        Err(_) => {}
        Ok(_) => return false,
    }

    true
}

// ---------------------------------------------------------------------------
// HONEST ternary benchmark: unified 3-way comparator vs naive binary.
// ---------------------------------------------------------------------------

/// Honest measurement of where (if anywhere) the unified 3-way comparator does
/// less work than a naive binary scheme. Returns `(unified_primitive_compares,
/// naive_primitive_compares)` for a workload that needs *3-way* decisions.
///
/// METHODOLOGY (kept rigorous so the number is defensible):
///   * We run the SAME insertion-sort over the SAME data in both arms, with the
///     compare at the SAME site. The only thing that differs is how a "less /
///     equal / greater" decision is obtained.
///   * BINARY ordering-only (e.g. a plain `<`-based sort) is genuinely ONE test
///     per step — there is NO ternary win there, and we do not pretend one.
///   * The win appears only when a step needs the FULL 3-way answer (a switch on
///     sign: do X if <, Y if ==, Z if >). Binary code must issue a `<` AND then
///     an `==` (two primitive compares); the ternary path issues ONE `cmp3` and
///     branches on its {-1,0,+1}. The classify phase below is exactly that case.
///
/// So the reported delta is attributable SOLELY to 3-way dispatch, not to a
/// rigged sort. If the workload only ever needed a 2-way `<`, the counts would
/// be equal — see `unified_sort == naive_sort` in the breakdown.
pub fn ternary_bench() -> (u64, u64) {
    let data: [f64; 16] = [
        9.0, 3.0, 7.0, 1.0, 8.0, 2.0, 6.0, 5.0, 4.0, 0.0, 9.0, 3.0, 2.0, 7.0, 1.0, 5.0,
    ];

    // ---- Phase 1: pure ordering sort. Identical algorithm both ways. ----
    // Unified arm: derive `>` from cmp3 sign.
    CMP_OPS.0.set(0);
    let mut uv: Vec<Value> = data.iter().map(|x| Value::Num(*x)).collect();
    insertion_sort_cmp3(&mut uv);
    let unified_sort = CMP_OPS.0.get();

    // Naive arm: same algorithm, one `<` per step. Count primitive compares.
    let mut naive_sort: u64 = 0;
    let mut nv: Vec<f64> = data.to_vec();
    {
        let mut i = 1;
        while i < nv.len() {
            let mut j = i;
            while j > 0 {
                naive_sort += 1; // ONE `<` — no ternary advantage here, honest.
                if !(nv[j - 1] > nv[j]) {
                    break;
                }
                nv.swap(j - 1, j);
                j -= 1;
            }
            i += 1;
        }
    }
    // (unified_sort == naive_sort is the honesty check: ordering-only ties.)

    // ---- Phase 2: 3-way classify of every adjacent pair (the real test). ----
    // Unified: ONE cmp3 yields the trit we branch on.
    let before = CMP_OPS.0.get();
    let mut sig = 0i64;
    for w in uv.windows(2) {
        match cmp3(&w[0], &w[1]).unwrap() {
            -1 => sig -= 1,
            0 => {}
            _ => sig += 1,
        }
    }
    let unified_classify = CMP_OPS.0.get() - before;
    let _ = sig;

    // Naive binary: a switch on sign needs `<` AND `==` (two primitive compares).
    let mut naive_classify: u64 = 0;
    for w in nv.windows(2) {
        naive_classify += 1; // `<`
        if w[0] < w[1] {
            // less
        } else {
            naive_classify += 1; // `==`
            let _ = w[0] == w[1]; // equal vs greater
        }
    }

    (
        unified_sort + unified_classify,
        naive_sort + naive_classify,
    )
}

/// Returns the per-phase breakdown for reporting:
/// (unified_sort, naive_sort, unified_classify, naive_classify).
pub fn ternary_bench_breakdown() -> (u64, u64, u64, u64) {
    let data: [f64; 16] = [
        9.0, 3.0, 7.0, 1.0, 8.0, 2.0, 6.0, 5.0, 4.0, 0.0, 9.0, 3.0, 2.0, 7.0, 1.0, 5.0,
    ];
    CMP_OPS.0.set(0);
    let mut uv: Vec<Value> = data.iter().map(|x| Value::Num(*x)).collect();
    insertion_sort_cmp3(&mut uv);
    let unified_sort = CMP_OPS.0.get();

    let mut naive_sort: u64 = 0;
    let mut nv: Vec<f64> = data.to_vec();
    let mut i = 1;
    while i < nv.len() {
        let mut j = i;
        while j > 0 {
            naive_sort += 1;
            if !(nv[j - 1] > nv[j]) {
                break;
            }
            nv.swap(j - 1, j);
            j -= 1;
        }
        i += 1;
    }

    let before = CMP_OPS.0.get();
    for w in uv.windows(2) {
        let _ = cmp3(&w[0], &w[1]).unwrap();
    }
    let unified_classify = CMP_OPS.0.get() - before;

    let mut naive_classify: u64 = 0;
    for w in nv.windows(2) {
        naive_classify += 1;
        if !(w[0] < w[1]) {
            naive_classify += 1;
            let _ = w[0] == w[1];
        }
    }
    (unified_sort, naive_sort, unified_classify, naive_classify)
}

// insertion sort whose ordering decision comes from cmp3's sign (one cmp/step).
fn insertion_sort_cmp3(v: &mut [Value]) {
    let mut i = 1;
    while i < v.len() {
        let mut j = i;
        while j > 0 {
            if cmp3(&v[j - 1], &v[j]).unwrap() <= 0 {
                break;
            }
            v.swap(j - 1, j);
            j -= 1;
        }
        i += 1;
    }
}
