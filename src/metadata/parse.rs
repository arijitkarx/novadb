//! A tiny recursive-descent parser for the NovaDB filter DSL.
//!
//! Grammar (matching the examples in task.md):
//!
//! ```text
//! expr    := term ( "AND" term )*
//! term    := ident op value
//! op      := "=" | "!=" | "<" | ">" | "<=" | ">="
//! value   := number | string | "true" | "false" | "null"
//! ident   := [a-zA-Z_][a-zA-Z0-9_]*
//! ```

use crate::error::{NovaError, Result};
use crate::metadata::filter::{Cmp, Filter};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Ident(String),
    Str(String),
    Num(f64),
    NumInteger(i64),
    Bool(bool),
    Null,
    Op(Cmp),
    And,
}

struct Lexer<'a> {
    input: &'a [u8],
    pos: usize,
}

impl<'a> Lexer<'a> {
    fn new(input: &'a str) -> Self {
        Lexer {
            input: input.as_bytes(),
            pos: 0,
        }
    }

    fn peek(&self) -> Option<u8> {
        self.input.get(self.pos).copied()
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.pos += 1;
        }
    }

    fn next_token(&mut self) -> Option<Result<Token>> {
        self.skip_ws();
        let c = self.peek()?;
        let token = match c {
            b'"' => self.lex_string(),
            b'-' | b'0'..=b'9' => self.lex_number(),
            b'a'..=b'z' | b'A'..=b'Z' | b'_' => self.lex_ident(),
            b'=' => {
                self.pos += 1;
                Ok(Token::Op(Cmp::Eq))
            }
            b'!' => {
                self.pos += 1;
                if self.peek() == Some(b'=') {
                    self.pos += 1;
                    Ok(Token::Op(Cmp::Ne))
                } else {
                    Err(self.err("expected '=' after '!'"))
                }
            }
            b'<' => {
                self.pos += 1;
                if self.peek() == Some(b'=') {
                    self.pos += 1;
                    Ok(Token::Op(Cmp::Lte))
                } else {
                    Ok(Token::Op(Cmp::Lt))
                }
            }
            b'>' => {
                self.pos += 1;
                if self.peek() == Some(b'=') {
                    self.pos += 1;
                    Ok(Token::Op(Cmp::Gte))
                } else {
                    Ok(Token::Op(Cmp::Gt))
                }
            }
            other => Err(self.err(&format!("unexpected character '{}'", other as char))),
        };
        Some(token)
    }

    fn lex_string(&mut self) -> Result<Token> {
        self.pos += 1; // opening quote
        let mut out = Vec::new();
        loop {
            let c = self.peek().ok_or_else(|| self.err("unterminated string"))?;
            self.pos += 1;
            match c {
                b'"' => {
                    return Ok(Token::Str(
                        String::from_utf8(out).map_err(|_| self.err("invalid utf-8 in string"))?,
                    ))
                }
                b'\\' => {
                    let esc = self.peek().ok_or_else(|| self.err("unterminated escape"))?;
                    self.pos += 1;
                    match esc {
                        b'"' => out.push(b'"'),
                        b'\\' => out.push(b'\\'),
                        b'n' => out.push(b'\n'),
                        b't' => out.push(b'\t'),
                        b'r' => out.push(b'\r'),
                        _ => return Err(self.err(&format!("invalid escape '\\{}'", esc as char))),
                    }
                }
                other => out.push(other),
            }
        }
    }

    fn lex_number(&mut self) -> Result<Token> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        while matches!(self.peek(), Some(b'0'..=b'9' | b'.')) {
            self.pos += 1;
        }
        let text = std::str::from_utf8(&self.input[start..self.pos])
            .map_err(|_| self.err("invalid number"))?;
        let n: f64 = text
            .parse()
            .map_err(|_| self.err(&format!("invalid number '{text}'")))?;
        // Integral literals become JSON integers (1 vs 1.0), which keeps
        // `price < 1000` matching metadata that stores integers.
        if n.fract() == 0.0 && n.is_finite() && n.abs() <= i64::MAX as f64 {
            Ok(Token::NumInteger(n as i64))
        } else {
            Ok(Token::Num(n))
        }
    }

    fn lex_ident(&mut self) -> Result<Token> {
        let start = self.pos;
        while matches!(
            self.peek(),
            Some(b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_')
        ) {
            self.pos += 1;
        }
        let text = std::str::from_utf8(&self.input[start..self.pos]).unwrap();
        let token = match text {
            "AND" | "and" => Token::And,
            "true" => Token::Bool(true),
            "false" => Token::Bool(false),
            "null" => Token::Null,
            _ => Token::Ident(text.to_string()),
        };
        Ok(token)
    }

    fn err(&self, msg: &str) -> NovaError {
        NovaError::Parse(format!("at position {}: {msg}", self.pos))
    }
}

/// Recursive-descent parser for the filter DSL.
pub struct Parser<'a> {
    lexer: Lexer<'a>,
    /// Lookahead of one token.
    pending: Option<Result<Token>>,
}

impl<'a> Parser<'a> {
    pub fn new(input: &'a str) -> Self {
        Parser {
            lexer: Lexer::new(input),
            pending: None,
        }
    }

    fn next(&mut self) -> Option<Result<Token>> {
        if let Some(tok) = self.pending.take() {
            return Some(tok);
        }
        self.lexer.next_token()
    }

    fn peek(&mut self) -> Option<&Result<Token>> {
        if self.pending.is_none() {
            self.pending = self.lexer.next_token();
        }
        self.pending.as_ref()
    }

    /// Parse a full expression: `term ("AND" term)*`.
    pub fn parse(&mut self) -> Result<Filter> {
        let mut terms = vec![self.parse_term()?];
        while let Some(Ok(Token::And)) = self.peek() {
            self.next(); // consume AND
            terms.push(self.parse_term()?);
        }
        Ok(match terms.len() {
            1 => terms.pop().unwrap(),
            _ => Filter::And(terms),
        })
    }

    fn parse_term(&mut self) -> Result<Filter> {
        let field = match self.next() {
            Some(Ok(Token::Ident(name))) => name,
            Some(Ok(other)) => return Err(self.err(&format!("expected field name, got {other:?}"))),
            Some(Err(e)) => return Err(e),
            None => return Err(self.err("expected a filter term")),
        };
        let op = match self.next() {
            Some(Ok(Token::Op(op))) => op,
            _ => return Err(self.err(&format!("expected comparison operator after '{field}'"))),
        };
        let value = match self.next() {
            Some(Ok(Token::Str(s))) => Value::String(s),
            Some(Ok(Token::Num(n))) => Value::from(n),
            Some(Ok(Token::NumInteger(n))) => Value::from(n),
            Some(Ok(Token::Bool(b))) => Value::Bool(b),
            Some(Ok(Token::Null)) => Value::Null,
            _ => return Err(self.err("expected a value after operator")),
        };
        Ok(Filter::Compare(field, op, value))
    }

    fn err(&self, msg: &str) -> NovaError {
        NovaError::Parse(msg.to_string())
    }

    /// Return any unconsumed input after `parse()` completed (whitespace only).
    pub fn remaining(&self) -> Option<&str> {
        let input = std::str::from_utf8(self.lexer.input).ok()?;
        let mut pos = self.lexer.pos;
        while matches!(
            self.lexer.input.get(pos),
            Some(b' ' | b'\t' | b'\n' | b'\r')
        ) {
            pos += 1;
        }
        let rest = &input[pos..];
        if rest.is_empty() {
            None
        } else {
            Some(rest)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::Filter;

    fn parse(dsl: &str) -> Result<Filter> {
        Filter::parse(dsl)
    }

    #[test]
    fn single_equality() {
        let f = parse(r#"category = "electronics""#).unwrap();
        assert_eq!(f, Filter::eq("category", "electronics"));
    }

    #[test]
    fn all_operators() {
        assert_eq!(parse("a < 1").unwrap(), Filter::lt("a", 1));
        assert_eq!(parse("a > 1").unwrap(), Filter::gt("a", 1));
        assert_eq!(parse("a <= 1").unwrap(), Filter::lte("a", 1));
        assert_eq!(parse("a >= 1").unwrap(), Filter::gte("a", 1));
        assert_eq!(parse("a != 1").unwrap(), Filter::ne("a", 1));
    }

    #[test]
    fn literals() {
        assert_eq!(parse("x = true").unwrap(), Filter::eq("x", true));
        assert_eq!(parse("x = false").unwrap(), Filter::eq("x", false));
        assert_eq!(parse("x = null").unwrap(), Filter::eq("x", Value::Null));
        assert_eq!(parse("x = -3.5").unwrap(), Filter::eq("x", -3.5));
    }

    #[test]
    fn and_chaining() {
        let f = parse("a = 1 AND b < 2 AND c >= 3").unwrap();
        assert_eq!(
            f,
            Filter::And(vec![
                Filter::eq("a", 1),
                Filter::lt("b", 2),
                Filter::gte("c", 3),
            ])
        );
    }

    #[test]
    fn string_escapes() {
        let f = parse(r#"name = "a\"b\\c""#).unwrap();
        assert_eq!(f, Filter::eq("name", r#"a"b\c"#));
    }

    #[test]
    fn malformed_inputs_rejected() {
        assert!(parse("").is_err());
        assert!(parse("a").is_err());
        assert!(parse("= 1").is_err());
        assert!(parse("a =").is_err());
        assert!(parse("a <").is_err());
        assert!(parse("a = 1 AND").is_err());
        assert!(parse("a = 1 b = 2").is_err()); // missing AND
        assert!(parse(r#"a = "unterminated"#).is_err());
        assert!(parse("a ~ 1").is_err());
    }

    #[test]
    fn number_without_int_part() {
        assert!(parse("a = .5").is_err());
    }
}
