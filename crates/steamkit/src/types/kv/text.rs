//! The text KeyValues parser.

use super::{KeyValue, ParseError};

pub(super) struct TextParser<'a> {
    pub(super) src: &'a [u8],
    pub(super) pos: usize,
}

enum Token {
    Str(String),
    Open,
    Close,
}

impl TextParser<'_> {
    pub(super) fn error(&self, message: &'static str) -> ParseError {
        ParseError { offset: self.pos, message }
    }

    fn skip_ws_and_comments(&mut self) {
        loop {
            while self.pos < self.src.len() && self.src[self.pos].is_ascii_whitespace() {
                self.pos += 1;
            }
            if self.src[self.pos..].starts_with(b"//") {
                while self.pos < self.src.len() && self.src[self.pos] != b'\n' {
                    self.pos += 1;
                }
                continue;
            }
            break;
        }
    }

    /// Skips a `[$CONDITION]` after a token. Conditions are not evaluated.
    fn skip_condition(&mut self) {
        let save = self.pos;
        self.skip_ws_and_comments();
        if self.src.get(self.pos) == Some(&b'[') {
            if let Some(end) = self.src[self.pos..].iter().position(|&b| b == b']') {
                self.pos += end + 1;
                return;
            }
        }
        self.pos = save;
    }

    fn next_token(&mut self) -> Result<Option<Token>, ParseError> {
        self.skip_ws_and_comments();
        let Some(&b) = self.src.get(self.pos) else {
            return Ok(None);
        };
        match b {
            b'{' => {
                self.pos += 1;
                Ok(Some(Token::Open))
            }
            b'}' => {
                self.pos += 1;
                Ok(Some(Token::Close))
            }
            b'"' => {
                self.pos += 1;
                let mut buf = Vec::new();
                loop {
                    let Some(&c) = self.src.get(self.pos) else {
                        return Err(self.error("unterminated string"));
                    };
                    self.pos += 1;
                    match c {
                        b'"' => break,
                        b'\\' => {
                            let Some(&e) = self.src.get(self.pos) else {
                                return Err(self.error("unterminated escape"));
                            };
                            self.pos += 1;
                            match e {
                                b'n' => buf.push(b'\n'),
                                b't' => buf.push(b'\t'),
                                b'\\' => buf.push(b'\\'),
                                b'"' => buf.push(b'"'),
                                other => {
                                    buf.push(b'\\');
                                    buf.push(other);
                                }
                            }
                        }
                        c => buf.push(c),
                    }
                }
                let s = String::from_utf8(buf).map_err(|_| self.error("invalid UTF-8"))?;
                Ok(Some(Token::Str(s)))
            }
            _ => {
                let start = self.pos;
                while let Some(&c) = self.src.get(self.pos) {
                    if c.is_ascii_whitespace() || matches!(c, b'{' | b'}' | b'"') {
                        break;
                    }
                    self.pos += 1;
                }
                let s = std::str::from_utf8(&self.src[start..self.pos]).map_err(|_| self.error("invalid UTF-8"))?;
                Ok(Some(Token::Str(s.to_string())))
            }
        }
    }

    /// Reads `key value` or `key { ... }`. `None` at end of input.
    pub(super) fn read_pair(&mut self) -> Result<Option<KeyValue>, ParseError> {
        loop {
            let Some(token) = self.next_token()? else {
                return Ok(None);
            };
            let name = match token {
                Token::Str(s) => s,
                Token::Open => return Err(self.error("unexpected '{'")),
                Token::Close => return Err(self.error("unexpected '}'")),
            };
            // `#include` / `#base` directives are not followed.
            if name == "#include" || name == "#base" {
                self.next_token()?;
                continue;
            }
            let kv = match self.next_token()? {
                Some(Token::Str(value)) => {
                    self.skip_condition();
                    KeyValue::new_string(name, value)
                }
                Some(Token::Open) => {
                    let children = self.read_children()?;
                    self.skip_condition();
                    KeyValue::new_object(name, children)
                }
                Some(Token::Close) => return Err(self.error("unexpected '}' after key")),
                None => return Err(self.error("unexpected end after key")),
            };
            return Ok(Some(kv));
        }
    }

    fn read_children(&mut self) -> Result<Vec<KeyValue>, ParseError> {
        let mut children = Vec::new();
        loop {
            self.skip_ws_and_comments();
            match self.src.get(self.pos) {
                None => return Err(self.error("missing '}'")),
                Some(b'}') => {
                    self.pos += 1;
                    return Ok(children);
                }
                Some(_) => match self.read_pair()? {
                    Some(kv) => children.push(kv),
                    None => return Err(self.error("missing '}'")),
                },
            }
        }
    }
}
