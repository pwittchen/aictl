//! Evaluate a mathematical expression safely without any `eval` or shell
//! subprocess. The model hands in a plain expression like `2 + 3 * 4`,
//! `sqrt(16) + sin(pi/2)`, or `(1 + 2) ^ 10`, and the tool returns the
//! numerical result as a string.
//!
//! Intentionally narrow scope: no variables, no assignments, no
//! user-defined functions, no string concatenation. The parser is a
//! recursive-descent implementation over a fixed grammar; the only side
//! effect is arithmetic. Recursion depth is bounded so a pathological
//! expression like `((((...))))` cannot blow the stack.
//!
//! Supported numeric literals: integers (`42`), decimals (`1.5`),
//! scientific notation (`1e5`, `2.5e-3`), hex (`0x1f`), binary (`0b1010`).
//!
//! Supported operators: unary `-` / `+`, `^` or `**` (power,
//! right-associative), `*`, `/`, `%` (remainder), `+`, `-`. Parentheses
//! for grouping.
//!
//! Supported constants: `pi`, `e`, `tau` (case-insensitive).
//!
//! Supported functions (case-insensitive): `sqrt`, `cbrt`, `abs`, `exp`,
//! `ln`, `log2`, `log10`, `log` (alias for `log10`), `sin`, `cos`, `tan`,
//! `asin`, `acos`, `atan`, `sinh`, `cosh`, `tanh`, `floor`, `ceil`,
//! `round`, `trunc`, `sign`. Two-argument: `min`, `max`, `pow`, `atan2`.
//!
//! Integer-valued results are rendered without a decimal point (`7`),
//! non-integer results use Rust's shortest-round-trip float formatting
//! (`2.5`, `0.30000000000000004`). `inf` / `nan` / `-inf` are returned
//! verbatim.

const MAX_DEPTH: usize = 128;

pub(super) fn tool_calculate(input: &str) -> String {
    let expr = input.trim();
    if expr.is_empty() {
        return "Error: empty expression".to_string();
    }
    if expr.contains('\0') {
        return "Error: expression contains null byte".to_string();
    }
    let normalized = normalize_operators(expr);
    match evaluate(&normalized) {
        Ok(v) => format_result(v),
        Err(e) => format!("Error: {e}"),
    }
}

/// Rewrite typographic math operators to their ASCII equivalents so an LLM
/// (or a user) can paste natural-language expressions like `3 × 4` or
/// `12 ÷ 5 − 1` without us needing to teach the tokenizer about every
/// Unicode codepoint.
fn normalize_operators(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '\u{00D7}' | '\u{22C5}' | '\u{2217}' | '\u{2A2F}' => '*', // × ⋅ ∗ ⨯
            '\u{00F7}' | '\u{2215}' | '\u{2044}' => '/',              // ÷ ∕ ⁄
            '\u{2212}' | '\u{2013}' | '\u{2014}' => '-',              // − – —
            _ => c,
        })
        .collect()
}

fn format_result(v: f64) -> String {
    if v.is_nan() {
        return "nan".to_string();
    }
    if v.is_infinite() {
        return if v > 0.0 { "inf" } else { "-inf" }.to_string();
    }
    if v.fract() == 0.0 && v.abs() < 1e18 {
        #[allow(clippy::cast_possible_truncation)]
        let i = v as i64;
        return i.to_string();
    }
    format!("{v}")
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(f64),
    Ident(String),
    LParen,
    RParen,
    Comma,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Caret,
    DoubleStar,
}

fn tokenize(s: &str) -> Result<Vec<Tok>, String> {
    let chars: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c.is_ascii_digit() || (c == '.' && i + 1 < chars.len() && chars[i + 1].is_ascii_digit())
        {
            let (tok, next) = read_number(&chars, i)?;
            out.push(tok);
            i = next;
            continue;
        }
        if c.is_alphabetic() || c == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let w: String = chars[start..i].iter().collect();
            out.push(Tok::Ident(w));
            continue;
        }
        match c {
            '(' => out.push(Tok::LParen),
            ')' => out.push(Tok::RParen),
            ',' => out.push(Tok::Comma),
            '+' => out.push(Tok::Plus),
            '-' => out.push(Tok::Minus),
            '*' => {
                if i + 1 < chars.len() && chars[i + 1] == '*' {
                    out.push(Tok::DoubleStar);
                    i += 2;
                    continue;
                }
                out.push(Tok::Star);
            }
            '/' => out.push(Tok::Slash),
            '%' => out.push(Tok::Percent),
            '^' => out.push(Tok::Caret),
            _ => return Err(format!("unexpected character '{c}'")),
        }
        i += 1;
    }
    Ok(out)
}

fn read_number(chars: &[char], mut i: usize) -> Result<(Tok, usize), String> {
    let start = i;
    let c = chars[i];

    if c == '0' && i + 1 < chars.len() && (chars[i + 1] == 'x' || chars[i + 1] == 'X') {
        i += 2;
        let h_start = i;
        while i < chars.len() && chars[i].is_ascii_hexdigit() {
            i += 1;
        }
        if i == h_start {
            return Err("empty hex literal '0x'".to_string());
        }
        let hex: String = chars[h_start..i].iter().collect();
        let n = i64::from_str_radix(&hex, 16).map_err(|e| format!("invalid hex literal: {e}"))?;
        #[allow(clippy::cast_precision_loss)]
        return Ok((Tok::Num(n as f64), i));
    }

    if c == '0' && i + 1 < chars.len() && (chars[i + 1] == 'b' || chars[i + 1] == 'B') {
        i += 2;
        let b_start = i;
        while i < chars.len() && (chars[i] == '0' || chars[i] == '1') {
            i += 1;
        }
        if i == b_start {
            return Err("empty binary literal '0b'".to_string());
        }
        let bin: String = chars[b_start..i].iter().collect();
        let n = i64::from_str_radix(&bin, 2).map_err(|e| format!("invalid binary literal: {e}"))?;
        #[allow(clippy::cast_precision_loss)]
        return Ok((Tok::Num(n as f64), i));
    }

    while i < chars.len() && chars[i].is_ascii_digit() {
        i += 1;
    }
    if i < chars.len() && chars[i] == '.' {
        i += 1;
        while i < chars.len() && chars[i].is_ascii_digit() {
            i += 1;
        }
    }
    if i < chars.len() && (chars[i] == 'e' || chars[i] == 'E') {
        i += 1;
        if i < chars.len() && (chars[i] == '+' || chars[i] == '-') {
            i += 1;
        }
        let exp_start = i;
        while i < chars.len() && chars[i].is_ascii_digit() {
            i += 1;
        }
        if i == exp_start {
            return Err("malformed exponent in numeric literal".to_string());
        }
    }
    let lit: String = chars[start..i].iter().collect();
    let n: f64 = lit
        .parse()
        .map_err(|e| format!("invalid number '{lit}': {e}"))?;
    Ok((Tok::Num(n), i))
}

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
    depth: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    fn next(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.pos).cloned();
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    fn enter(&mut self) -> Result<(), String> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err("expression nesting too deep".to_string());
        }
        Ok(())
    }

    fn leave(&mut self) {
        self.depth -= 1;
    }
}

fn evaluate(expr: &str) -> Result<f64, String> {
    let toks = tokenize(expr)?;
    if toks.is_empty() {
        return Err("empty expression".to_string());
    }
    let mut p = Parser {
        toks,
        pos: 0,
        depth: 0,
    };
    let v = parse_expr(&mut p)?;
    if p.pos < p.toks.len() {
        let extra = p
            .peek()
            .map_or_else(String::new, |t| format!(" ({})", describe(t)));
        return Err(format!("unexpected token at position {}{extra}", p.pos + 1));
    }
    Ok(v)
}

fn parse_expr(p: &mut Parser) -> Result<f64, String> {
    p.enter()?;
    let mut left = parse_term(p)?;
    loop {
        match p.peek() {
            Some(Tok::Plus) => {
                p.next();
                left += parse_term(p)?;
            }
            Some(Tok::Minus) => {
                p.next();
                left -= parse_term(p)?;
            }
            _ => break,
        }
    }
    p.leave();
    Ok(left)
}

fn parse_term(p: &mut Parser) -> Result<f64, String> {
    p.enter()?;
    let mut left = parse_factor(p)?;
    loop {
        match p.peek() {
            Some(Tok::Star) => {
                p.next();
                left *= parse_factor(p)?;
            }
            Some(Tok::Slash) => {
                p.next();
                left /= parse_factor(p)?;
            }
            Some(Tok::Percent) => {
                p.next();
                left %= parse_factor(p)?;
            }
            _ => break,
        }
    }
    p.leave();
    Ok(left)
}

fn parse_factor(p: &mut Parser) -> Result<f64, String> {
    p.enter()?;
    let base = parse_unary(p)?;
    let r = if matches!(p.peek(), Some(Tok::Caret | Tok::DoubleStar)) {
        p.next();
        let exp = parse_factor(p)?;
        base.powf(exp)
    } else {
        base
    };
    p.leave();
    Ok(r)
}

fn parse_unary(p: &mut Parser) -> Result<f64, String> {
    p.enter()?;
    let v = match p.peek() {
        Some(Tok::Minus) => {
            p.next();
            -parse_unary(p)?
        }
        Some(Tok::Plus) => {
            p.next();
            parse_unary(p)?
        }
        _ => parse_primary(p)?,
    };
    p.leave();
    Ok(v)
}

fn parse_primary(p: &mut Parser) -> Result<f64, String> {
    match p.next() {
        Some(Tok::Num(n)) => Ok(n),
        Some(Tok::LParen) => {
            let v = parse_expr(p)?;
            match p.next() {
                Some(Tok::RParen) => Ok(v),
                Some(other) => Err(format!("expected ')', got {}", describe(&other))),
                None => Err("expected ')', got end of expression".to_string()),
            }
        }
        Some(Tok::Ident(name)) => {
            if matches!(p.peek(), Some(Tok::LParen)) {
                p.next();
                let mut args = Vec::new();
                if !matches!(p.peek(), Some(Tok::RParen)) {
                    loop {
                        args.push(parse_expr(p)?);
                        if matches!(p.peek(), Some(Tok::Comma)) {
                            p.next();
                        } else {
                            break;
                        }
                    }
                }
                match p.next() {
                    Some(Tok::RParen) => {}
                    Some(other) => {
                        return Err(format!(
                            "expected ')' to close call to '{name}', got {}",
                            describe(&other)
                        ));
                    }
                    None => {
                        return Err(format!(
                            "expected ')' to close call to '{name}', got end of expression"
                        ));
                    }
                }
                apply_function(&name, &args)
            } else {
                apply_constant(&name)
            }
        }
        Some(other) => Err(format!(
            "expected number, identifier, or '(', got {}",
            describe(&other)
        )),
        None => Err("unexpected end of expression".to_string()),
    }
}

fn describe(t: &Tok) -> String {
    match t {
        Tok::Num(n) => format!("number {n}"),
        Tok::Ident(s) => format!("identifier '{s}'"),
        Tok::LParen => "'('".to_string(),
        Tok::RParen => "')'".to_string(),
        Tok::Comma => "','".to_string(),
        Tok::Plus => "'+'".to_string(),
        Tok::Minus => "'-'".to_string(),
        Tok::Star => "'*'".to_string(),
        Tok::Slash => "'/'".to_string(),
        Tok::Percent => "'%'".to_string(),
        Tok::Caret => "'^'".to_string(),
        Tok::DoubleStar => "'**'".to_string(),
    }
}

fn apply_constant(name: &str) -> Result<f64, String> {
    match name.to_ascii_lowercase().as_str() {
        "pi" => Ok(std::f64::consts::PI),
        "e" => Ok(std::f64::consts::E),
        "tau" => Ok(std::f64::consts::TAU),
        _ => Err(format!(
            "unknown identifier '{name}' (not a constant; did you forget '(' for a function call?)"
        )),
    }
}

fn apply_function(name: &str, args: &[f64]) -> Result<f64, String> {
    let n = name.to_ascii_lowercase();
    if let Some(f) = one_arg_fn(&n) {
        return match args {
            [x] => Ok(f(*x)),
            _ => Err(format!(
                "function '{name}' expects 1 argument, got {}",
                args.len()
            )),
        };
    }
    if let Some(f) = two_arg_fn(&n) {
        return match args {
            [a, b] => Ok(f(*a, *b)),
            _ => Err(format!(
                "function '{name}' expects 2 arguments, got {}",
                args.len()
            )),
        };
    }
    Err(format!("unknown function '{name}'"))
}

fn one_arg_fn(n: &str) -> Option<fn(f64) -> f64> {
    Some(match n {
        "sqrt" => f64::sqrt,
        "cbrt" => f64::cbrt,
        "abs" => f64::abs,
        "exp" => f64::exp,
        "ln" => f64::ln,
        "log2" => f64::log2,
        "log10" | "log" => f64::log10,
        "sin" => f64::sin,
        "cos" => f64::cos,
        "tan" => f64::tan,
        "asin" => f64::asin,
        "acos" => f64::acos,
        "atan" => f64::atan,
        "sinh" => f64::sinh,
        "cosh" => f64::cosh,
        "tanh" => f64::tanh,
        "floor" => f64::floor,
        "ceil" => f64::ceil,
        "round" => f64::round,
        "trunc" => f64::trunc,
        "sign" => f64::signum,
        _ => return None,
    })
}

fn two_arg_fn(n: &str) -> Option<fn(f64, f64) -> f64> {
    Some(match n {
        "min" => f64::min,
        "max" => f64::max,
        "pow" => f64::powf,
        "atan2" => f64::atan2,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_rejected() {
        assert!(tool_calculate("").contains("empty expression"));
        assert!(tool_calculate("   ").contains("empty expression"));
    }

    #[test]
    fn null_byte_rejected() {
        assert!(tool_calculate("1+\01").contains("null byte"));
    }

    #[test]
    fn integer_arithmetic() {
        assert_eq!(tool_calculate("1+1"), "2");
        assert_eq!(tool_calculate("2 + 3 * 4"), "14");
        assert_eq!(tool_calculate("(2 + 3) * 4"), "20");
        assert_eq!(tool_calculate("10 - 2 - 3"), "5");
        assert_eq!(tool_calculate("100 / 4"), "25");
        assert_eq!(tool_calculate("10 % 3"), "1");
    }

    #[test]
    fn float_arithmetic() {
        assert_eq!(tool_calculate("2.5 + 2.5"), "5");
        assert_eq!(tool_calculate("1.5 * 2"), "3");
        assert_eq!(tool_calculate("0.1 + 0.2"), "0.30000000000000004");
    }

    #[test]
    fn unary_minus() {
        assert_eq!(tool_calculate("-5"), "-5");
        assert_eq!(tool_calculate("-(2+3)"), "-5");
        assert_eq!(tool_calculate("--5"), "5");
        assert_eq!(tool_calculate("+-5"), "-5");
        assert_eq!(tool_calculate("3 - -2"), "5");
    }

    #[test]
    fn power_right_associative() {
        assert_eq!(tool_calculate("2^3"), "8");
        assert_eq!(tool_calculate("2**3"), "8");
        assert_eq!(tool_calculate("2^3^2"), "512");
        assert_eq!(tool_calculate("(2^3)^2"), "64");
        assert_eq!(tool_calculate("-2^2"), "4");
    }

    #[test]
    fn precedence() {
        assert_eq!(tool_calculate("2 + 3 * 4"), "14");
        assert_eq!(tool_calculate("2 * 3 + 4"), "10");
        assert_eq!(tool_calculate("2 + 3 ^ 2"), "11");
        assert_eq!(tool_calculate("2 * 3 ^ 2"), "18");
    }

    #[test]
    fn division_by_zero_is_inf() {
        assert_eq!(tool_calculate("1/0"), "inf");
        assert_eq!(tool_calculate("-1/0"), "-inf");
        assert_eq!(tool_calculate("0/0"), "nan");
    }

    #[test]
    fn scientific_notation() {
        assert_eq!(tool_calculate("1e3"), "1000");
        assert_eq!(tool_calculate("1.5e2"), "150");
        assert_eq!(tool_calculate("1e-2"), "0.01");
    }

    #[test]
    fn hex_and_binary_literals() {
        assert_eq!(tool_calculate("0xff"), "255");
        assert_eq!(tool_calculate("0x10 + 1"), "17");
        assert_eq!(tool_calculate("0b1010"), "10");
        assert_eq!(tool_calculate("0b11 * 2"), "6");
    }

    #[test]
    fn constants() {
        assert!(tool_calculate("pi").starts_with("3.14"));
        assert!(tool_calculate("E").starts_with("2.71"));
        assert!(tool_calculate("tau").starts_with("6.28"));
    }

    #[test]
    fn functions_one_arg() {
        assert_eq!(tool_calculate("sqrt(16)"), "4");
        assert_eq!(tool_calculate("abs(-7)"), "7");
        assert_eq!(tool_calculate("floor(2.9)"), "2");
        assert_eq!(tool_calculate("ceil(2.1)"), "3");
        assert_eq!(tool_calculate("round(2.5)"), "3");
        assert_eq!(tool_calculate("trunc(2.9)"), "2");
    }

    #[test]
    fn functions_trig() {
        assert_eq!(tool_calculate("sin(0)"), "0");
        assert_eq!(tool_calculate("cos(0)"), "1");
        assert_eq!(tool_calculate("sin(pi/2)"), "1");
    }

    #[test]
    fn functions_log() {
        assert_eq!(tool_calculate("ln(e)"), "1");
        assert_eq!(tool_calculate("log10(1000)"), "3");
        assert_eq!(tool_calculate("log(100)"), "2");
        assert_eq!(tool_calculate("log2(8)"), "3");
    }

    #[test]
    fn functions_two_args() {
        assert_eq!(tool_calculate("min(3, 5)"), "3");
        assert_eq!(tool_calculate("max(3, 5)"), "5");
        assert_eq!(tool_calculate("pow(2, 10)"), "1024");
    }

    #[test]
    fn function_wrong_arity_reports_error() {
        assert!(tool_calculate("sqrt(1, 2)").contains("expects 1"));
        assert!(tool_calculate("min(1)").contains("expects 2"));
        assert!(tool_calculate("max()").contains("expects 2"));
    }

    #[test]
    fn unknown_function_reports_error() {
        assert!(tool_calculate("foo(1)").contains("unknown function"));
    }

    #[test]
    fn unknown_identifier_reports_error() {
        assert!(tool_calculate("x + 1").contains("unknown identifier"));
    }

    #[test]
    fn mismatched_parens() {
        assert!(tool_calculate("(1 + 2").starts_with("Error"));
        assert!(tool_calculate("1 + 2)").starts_with("Error"));
    }

    #[test]
    fn unexpected_character() {
        assert!(tool_calculate("1 $ 2").contains("unexpected character"));
    }

    #[test]
    fn trailing_tokens_rejected() {
        assert!(tool_calculate("1 + 2 3").contains("unexpected token"));
    }

    #[test]
    fn deeply_nested_rejected() {
        let expr = "(".repeat(500) + "1" + &")".repeat(500);
        assert!(tool_calculate(&expr).contains("too deep"));
    }

    #[test]
    fn whitespace_and_newlines_ok() {
        assert_eq!(tool_calculate("  1\n+\t2 "), "3");
    }

    #[test]
    fn integer_valued_float_renders_as_integer() {
        assert_eq!(tool_calculate("2.0 * 3.0"), "6");
        assert_eq!(tool_calculate("sqrt(9)"), "3");
    }

    #[test]
    fn unicode_multiplication_sign() {
        assert_eq!(tool_calculate("3 × 4"), "12");
        assert_eq!(tool_calculate("(2 + 3) × 4"), "20");
    }

    #[test]
    fn unicode_division_and_minus() {
        assert_eq!(tool_calculate("12 ÷ 4"), "3");
        assert_eq!(tool_calculate("10 − 3"), "7");
        assert_eq!(tool_calculate("6 × 4 ÷ 3 − 1"), "7");
    }

    #[test]
    fn negative_result_preserved() {
        assert_eq!(tool_calculate("3 - 10"), "-7");
        assert_eq!(tool_calculate("-0.5 * 2"), "-1");
    }
}
