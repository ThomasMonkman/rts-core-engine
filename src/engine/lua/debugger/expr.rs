//! The debugger's expressions: breakpoint conditions, hovers, the watch
//! list and the debug console. A read-only subset of Lua, evaluated raw on
//! the stopped stack (`inspect`), so nothing in the VM runs or changes:
//!
//! - names, fields and indexing: `self.Blueprint.General`, `t[i]`, `t["k"]`
//!   (`a:b` reads the field `b`; calls aren't allowed);
//! - literals: numbers, strings, `nil`, `true`, `false`;
//! - `== ~= < <= > >=`, `and or not`, `+ - * / %`, unary `-`, `#`.
//!
//! Lua's precedence and short-circuiting; no metamethods (`==` is raw, `#`
//! is the raw length). Hit counts (`hitCondition`) are [`HitCount`].

#[derive(Debug, Clone, PartialEq)]
pub(super) enum Expr {
    Nil,
    Bool(bool),
    Num(f64),
    Str(Vec<u8>),
    Name(String),
    Index(Box<Expr>, Box<Expr>),
    Unary(UnOp, Box<Expr>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum UnOp {
    Not,
    Neg,
    Len,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum BinOp {
    Or,
    And,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Add,
    Sub,
    Mul,
    Div,
    Mod,
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(f64),
    Str(Vec<u8>),
    Name(String),
    /// A symbol or a keyword.
    Sym(&'static str),
}

const KEYWORDS: [&str; 6] = ["and", "or", "not", "nil", "true", "false"];
const SYMBOLS: [&str; 19] = ["==", "~=", "<=", ">=", "..", "<", ">", "+", "-", "*", "/", "%", "#", "(", ")", "[", "]", ".", ":"];

fn lex(src: &str) -> Result<Vec<Tok>, String> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_whitespace() {
            i += 1;
        } else if c.is_ascii_digit() || (c == b'.' && b.get(i + 1).is_some_and(u8::is_ascii_digit)) {
            let start = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'.' || ((b[i] == b'+' || b[i] == b'-') && matches!(b[i - 1], b'e' | b'E') && !b[start..i].starts_with(b"0x"))) {
                i += 1;
            }
            let text = &src[start..i];
            let n = match text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
                Some(hex) => i64::from_str_radix(hex, 16).ok().map(|n| n as f64),
                None => text.parse::<f64>().ok(),
            };
            out.push(Tok::Num(n.ok_or_else(|| format!("bad number {text}"))?));
        } else if c == b'"' || c == b'\'' {
            i += 1;
            let mut s = Vec::new();
            loop {
                let Some(&ch) = b.get(i) else { return Err("unfinished string".into()) };
                i += 1;
                match ch {
                    _ if ch == c => break,
                    b'\\' => {
                        let Some(&e) = b.get(i) else { return Err("unfinished string".into()) };
                        i += 1;
                        s.push(match e {
                            b'n' => b'\n',
                            b't' => b'\t',
                            b'r' => b'\r',
                            b'0' => 0,
                            other => other,
                        });
                    }
                    _ => s.push(ch),
                }
            }
            out.push(Tok::Str(s));
        } else if c.is_ascii_alphabetic() || c == b'_' {
            let start = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            let word = &src[start..i];
            out.push(match KEYWORDS.iter().find(|k| **k == word) {
                Some(k) => Tok::Sym(k),
                None => Tok::Name(word.to_string()),
            });
        } else if let Some(sym) = SYMBOLS.iter().find(|s| b[i..].starts_with(s.as_bytes())) {
            i += sym.len();
            out.push(Tok::Sym(sym));
        } else {
            let ch = src[i..].chars().next().unwrap_or('?');
            return Err(format!("unexpected '{ch}'"));
        }
    }
    Ok(out)
}

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
    depth: u32,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    fn eat(&mut self, sym: &str) -> bool {
        if matches!(self.peek(), Some(Tok::Sym(s)) if *s == sym) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn binary(&mut self, ops: &[(&str, BinOp)], next: fn(&mut Self) -> Result<Expr, String>) -> Result<Expr, String> {
        let mut lhs = next(self)?;
        'outer: loop {
            for (sym, op) in ops {
                if self.eat(sym) {
                    let rhs = next(self)?;
                    lhs = Expr::Binary(*op, Box::new(lhs), Box::new(rhs));
                    continue 'outer;
                }
            }
            return Ok(lhs);
        }
    }

    fn expr(&mut self) -> Result<Expr, String> {
        self.depth += 1;
        if self.depth > 100 {
            return Err("expression nested too deeply".into());
        }
        let e = self.or();
        self.depth -= 1;
        e
    }

    fn or(&mut self) -> Result<Expr, String> {
        self.binary(&[("or", BinOp::Or)], Self::and)
    }

    fn and(&mut self) -> Result<Expr, String> {
        self.binary(&[("and", BinOp::And)], Self::cmp)
    }

    fn cmp(&mut self) -> Result<Expr, String> {
        let ops = [("==", BinOp::Eq), ("~=", BinOp::Ne), ("<=", BinOp::Le), (">=", BinOp::Ge), ("<", BinOp::Lt), (">", BinOp::Gt)];
        self.binary(&ops, Self::add)
    }

    fn add(&mut self) -> Result<Expr, String> {
        self.binary(&[("+", BinOp::Add), ("-", BinOp::Sub)], Self::mul)
    }

    fn mul(&mut self) -> Result<Expr, String> {
        self.binary(&[("*", BinOp::Mul), ("/", BinOp::Div), ("%", BinOp::Mod)], Self::unary)
    }

    fn unary(&mut self) -> Result<Expr, String> {
        for (sym, op) in [("not", UnOp::Not), ("-", UnOp::Neg), ("#", UnOp::Len)] {
            if self.eat(sym) {
                self.depth += 1;
                if self.depth > 100 {
                    return Err("expression nested too deeply".into());
                }
                let e = self.unary()?;
                self.depth -= 1;
                return Ok(Expr::Unary(op, Box::new(e)));
            }
        }
        self.postfix()
    }

    fn name(&mut self) -> Result<String, String> {
        match self.peek() {
            Some(Tok::Name(n)) => {
                let n = n.clone();
                self.pos += 1;
                Ok(n)
            }
            _ => Err("expected a name".into()),
        }
    }

    fn postfix(&mut self) -> Result<Expr, String> {
        let mut e = self.primary()?;
        loop {
            if self.eat(".") || self.eat(":") {
                e = Expr::Index(Box::new(e), Box::new(Expr::Str(self.name()?.into_bytes())));
            } else if self.eat("[") {
                let k = self.expr()?;
                if !self.eat("]") {
                    return Err("expected ']'".into());
                }
                e = Expr::Index(Box::new(e), Box::new(k));
            } else if matches!(self.peek(), Some(Tok::Sym("(")) | Some(Tok::Str(_))) {
                return Err("the debugger can't call functions (it only reads values)".into());
            } else {
                return Ok(e);
            }
        }
    }

    fn primary(&mut self) -> Result<Expr, String> {
        let Some(t) = self.peek().cloned() else { return Err("expected a value".into()) };
        self.pos += 1;
        Ok(match t {
            Tok::Num(n) => Expr::Num(n),
            Tok::Str(s) => Expr::Str(s),
            Tok::Name(n) => Expr::Name(n),
            Tok::Sym("nil") => Expr::Nil,
            Tok::Sym("true") => Expr::Bool(true),
            Tok::Sym("false") => Expr::Bool(false),
            Tok::Sym("(") => {
                let e = self.expr()?;
                if !self.eat(")") {
                    return Err("expected ')'".into());
                }
                e
            }
            Tok::Sym("..") => return Err("'..' isn't supported".into()),
            Tok::Sym(s) => return Err(format!("unexpected '{s}'")),
        })
    }
}

/// Parses an expression (see the module docs).
pub(super) fn parse(src: &str) -> Result<Expr, String> {
    let mut p = Parser { toks: lex(src)?, pos: 0, depth: 0 };
    let e = p.expr()?;
    match p.peek() {
        None => Ok(e),
        Some(Tok::Sym("..")) => Err("'..' isn't supported".into()),
        Some(_) => Err("unexpected text after the expression".into()),
    }
}

/// A breakpoint's `hitCondition`: `5` or `== 5` (the 5th hit), `>= 5`,
/// `> 5`, `< 5`, `<= 5`, `% 5` (every 5th). Hits are counted when the
/// condition (if any) holds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct HitCount {
    op: &'static str,
    n: u64,
}

impl HitCount {
    pub(super) fn parse(src: &str) -> Result<Self, String> {
        let s = src.trim();
        let (op, rest) = ["==", ">=", "<=", ">", "<", "%", "="].iter().find_map(|op| s.strip_prefix(op).map(|r| (*op, r))).unwrap_or(("==", s));
        let n: u64 = rest.trim().parse().map_err(|_| format!("hit count {src:?}: expected a number, optionally after == >= > <= < or %"))?;
        if op == "%" && n == 0 {
            return Err("hit count: % 0".into());
        }
        Ok(Self { op: if op == "=" { "==" } else { op }, n })
    }

    /// Whether the `hits`-th hit stops.
    pub(super) fn matches(&self, hits: u64) -> bool {
        match self.op {
            ">=" => hits >= self.n,
            ">" => hits > self.n,
            "<=" => hits <= self.n,
            "<" => hits < self.n,
            "%" => hits.is_multiple_of(self.n),
            _ => hits == self.n,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(n: &str) -> Box<Expr> {
        Box::new(Expr::Name(n.into()))
    }
    fn field(e: Box<Expr>, k: &str) -> Box<Expr> {
        Box::new(Expr::Index(e, Box::new(Expr::Str(k.into()))))
    }

    #[test]
    fn parses_paths() {
        assert_eq!(parse("self"), Ok(Expr::Name("self".into())));
        assert_eq!(parse("a.b[2][\"c d\"]:e"), Ok(*field(field(Box::new(Expr::Index(field(name("a"), "b"), Box::new(Expr::Num(2.0)))), "c d"), "e")));
        assert_eq!(parse("t[i]"), Ok(Expr::Index(name("t"), name("i"))));
    }

    #[test]
    fn follows_lua_precedence() {
        // `not` binds tighter than `==`; `and` tighter than `or`.
        assert_eq!(parse("not a == b"), Ok(Expr::Binary(BinOp::Eq, Box::new(Expr::Unary(UnOp::Not, name("a"))), name("b"))));
        assert_eq!(
            parse("a or b and c"),
            Ok(Expr::Binary(BinOp::Or, name("a"), Box::new(Expr::Binary(BinOp::And, name("b"), name("c")))))
        );
        assert_eq!(
            parse("1 + 2 * 3 > x"),
            Ok(Expr::Binary(
                BinOp::Gt,
                Box::new(Expr::Binary(BinOp::Add, Box::new(Expr::Num(1.0)), Box::new(Expr::Binary(BinOp::Mul, Box::new(Expr::Num(2.0)), Box::new(Expr::Num(3.0)))))),
                name("x")
            ))
        );
        assert_eq!(parse("self.UnitId == 'uel0001'"), Ok(Expr::Binary(BinOp::Eq, field(name("self"), "UnitId"), Box::new(Expr::Str(b"uel0001".to_vec())))));
        assert_eq!(parse("#t >= 0x10"), Ok(Expr::Binary(BinOp::Ge, Box::new(Expr::Unary(UnOp::Len, name("t"))), Box::new(Expr::Num(16.0)))));
        assert_eq!(parse("1e-3"), Ok(Expr::Num(0.001)));
    }

    #[test]
    fn rejects_what_it_cant_read() {
        assert!(parse("f()").unwrap_err().contains("call"));
        assert!(parse("self:GetHealth() < 10").unwrap_err().contains("call"));
        assert!(parse("a .. b").is_err());
        assert!(parse("a +").is_err());
        assert!(parse("a b").is_err());
        assert!(parse("'open").is_err());
        assert!(parse("1x").is_err());
    }

    #[test]
    fn hit_counts() {
        let h = |s| HitCount::parse(s).unwrap();
        assert!(h("3").matches(3) && !h("3").matches(4));
        assert!(h(">= 2").matches(2) && !h(">= 2").matches(1));
        assert!(h("%3").matches(6) && !h("%3").matches(7));
        assert!(h("< 2").matches(1) && !h("< 2").matches(2));
        assert!(HitCount::parse("abc").is_err() && HitCount::parse("% 0").is_err());
    }
}
