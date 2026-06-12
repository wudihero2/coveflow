//! Tokenizer for the restricted expression language.

use crate::ExprError;

/// Upper bound on tokens per expression. Bounds total work and, with the
/// parser/eval depth guards, prevents a pathological expression from exhausting
/// the (shared, non-sandboxed) process stack.
pub(crate) const MAX_TOKENS: usize = 4096;

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Token {
    Number(f64),
    Str(String),
    Ident(String),
    True,
    False,
    Null,
    Plus,
    Minus,
    Star,
    Slash,
    Bang,
    AndAnd,
    OrOr,
    EqEq,
    NotEq,
    Lt,
    Le,
    Gt,
    Ge,
    LParen,
    RParen,
    Dot,
    LBracket,
    RBracket,
    Question,
    Colon,
}

pub(crate) fn tokenize(src: &str) -> Result<Vec<Token>, ExprError> {
    let chars: Vec<char> = src.chars().collect();
    let mut i = 0;
    let mut out: Vec<Token> = Vec::new();
    while i < chars.len() {
        let c = chars[i];
        match c {
            ' ' | '\t' | '\n' | '\r' => i += 1,
            '+' => {
                out.push(Token::Plus);
                i += 1;
            }
            '-' => {
                out.push(Token::Minus);
                i += 1;
            }
            '*' => {
                out.push(Token::Star);
                i += 1;
            }
            '/' => {
                out.push(Token::Slash);
                i += 1;
            }
            '(' => {
                out.push(Token::LParen);
                i += 1;
            }
            ')' => {
                out.push(Token::RParen);
                i += 1;
            }
            '.' => {
                out.push(Token::Dot);
                i += 1;
            }
            '[' => {
                out.push(Token::LBracket);
                i += 1;
            }
            ']' => {
                out.push(Token::RBracket);
                i += 1;
            }
            '?' => {
                out.push(Token::Question);
                i += 1;
            }
            ':' => {
                out.push(Token::Colon);
                i += 1;
            }
            '!' => {
                if chars.get(i + 1) == Some(&'=') {
                    out.push(Token::NotEq);
                    i += 2;
                } else {
                    out.push(Token::Bang);
                    i += 1;
                }
            }
            '=' => {
                if chars.get(i + 1) == Some(&'=') {
                    out.push(Token::EqEq);
                    i += 2;
                } else {
                    return Err(ExprError::Parse("'=' is not an operator (use '==')".into()));
                }
            }
            '<' => {
                if chars.get(i + 1) == Some(&'=') {
                    out.push(Token::Le);
                    i += 2;
                } else {
                    out.push(Token::Lt);
                    i += 1;
                }
            }
            '>' => {
                if chars.get(i + 1) == Some(&'=') {
                    out.push(Token::Ge);
                    i += 2;
                } else {
                    out.push(Token::Gt);
                    i += 1;
                }
            }
            '&' => {
                if chars.get(i + 1) == Some(&'&') {
                    out.push(Token::AndAnd);
                    i += 2;
                } else {
                    return Err(ExprError::Parse("single '&' is not allowed".into()));
                }
            }
            '|' => {
                if chars.get(i + 1) == Some(&'|') {
                    out.push(Token::OrOr);
                    i += 2;
                } else {
                    return Err(ExprError::Parse("single '|' is not allowed".into()));
                }
            }
            '"' | '\'' => {
                let quote = c;
                let mut s = String::new();
                i += 1;
                let mut closed = false;
                while i < chars.len() {
                    let ch = chars[i];
                    if ch == '\\' {
                        // Minimal escapes: \" \' \\ \n \t
                        if let Some(&next) = chars.get(i + 1) {
                            let mapped = match next {
                                'n' => '\n',
                                't' => '\t',
                                '\\' => '\\',
                                '"' => '"',
                                '\'' => '\'',
                                other => other,
                            };
                            s.push(mapped);
                            i += 2;
                            continue;
                        }
                        return Err(ExprError::Parse("trailing backslash in string".into()));
                    }
                    if ch == quote {
                        closed = true;
                        i += 1;
                        break;
                    }
                    s.push(ch);
                    i += 1;
                }
                if !closed {
                    return Err(ExprError::Parse("unterminated string literal".into()));
                }
                out.push(Token::Str(s));
            }
            c if c.is_ascii_digit() => {
                let start = i;
                let mut seen_dot = false;
                while i < chars.len() {
                    let ch = chars[i];
                    if ch.is_ascii_digit() {
                        i += 1;
                    } else if ch == '.'
                        && !seen_dot
                        && chars.get(i + 1).is_some_and(|n| n.is_ascii_digit())
                    {
                        // Only consume '.' as a decimal point when followed by a
                        // digit, so `a.0` style field access still lexes as Dot.
                        seen_dot = true;
                        i += 1;
                    } else {
                        break;
                    }
                }
                let lit: String = chars[start..i].iter().collect();
                let n = lit
                    .parse::<f64>()
                    .map_err(|_| ExprError::Parse(format!("invalid number '{lit}'")))?;
                out.push(Token::Number(n));
            }
            c if c.is_ascii_alphabetic() || c == '_' => {
                let start = i;
                while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
                let word: String = chars[start..i].iter().collect();
                out.push(match word.as_str() {
                    "true" => Token::True,
                    "false" => Token::False,
                    "null" => Token::Null,
                    _ => Token::Ident(word),
                });
            }
            other => {
                return Err(ExprError::Parse(format!("unexpected character '{other}'")));
            }
        }
        if out.len() > MAX_TOKENS {
            return Err(ExprError::Parse("expression too long".into()));
        }
    }
    Ok(out)
}
