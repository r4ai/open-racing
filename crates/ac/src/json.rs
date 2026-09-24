//! Lenient JSON reader for the game's UI files, which are hand-edited: they may hold
//! trailing commas, comments and raw control characters in strings.

use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<Value>),
    Object(BTreeMap<String, Value>),
}

impl Value {
    /// The member `key` of an object, compared case-insensitively.
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Self::Object(m) => m
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(key))
                .map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Number(n) => Some(*n),
            _ => None,
        }
    }

    pub fn as_array(&self) -> &[Value] {
        match self {
            Self::Array(a) => a,
            _ => &[],
        }
    }
}

struct Parser<'a> {
    src: &'a [u8],
    pos: usize,
}

pub fn parse(src: &str) -> Result<Value, String> {
    let src = src.strip_prefix('\u{feff}').unwrap_or(src);
    let mut p = Parser {
        src: src.as_bytes(),
        pos: 0,
    };
    let v = p.value(0)?;
    p.skip_space();
    if p.pos < p.src.len() {
        return Err(format!("unexpected data at byte {}", p.pos));
    }
    Ok(v)
}

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.src.get(self.pos).copied()
    }

    fn skip_space(&mut self) {
        loop {
            match self.peek() {
                Some(c) if c.is_ascii_whitespace() => self.pos += 1,
                Some(b'/') if self.src.get(self.pos + 1) == Some(&b'/') => {
                    while self.peek().is_some_and(|c| c != b'\n') {
                        self.pos += 1;
                    }
                }
                _ => return,
            }
        }
    }

    fn error(&self, what: &str) -> String {
        format!("{what} at byte {}", self.pos)
    }

    fn value(&mut self, depth: usize) -> Result<Value, String> {
        if depth > 64 {
            return Err(self.error("nesting too deep"));
        }
        self.skip_space();
        match self.peek() {
            Some(b'{') => self.object(depth),
            Some(b'[') => self.array(depth),
            Some(b'"') => Ok(Value::String(self.string()?)),
            Some(b't') => self.word("true", Value::Bool(true)),
            Some(b'f') => self.word("false", Value::Bool(false)),
            Some(b'n') => self.word("null", Value::Null),
            Some(c) if c == b'-' || c.is_ascii_digit() => self.number(),
            _ => Err(self.error("expected a value")),
        }
    }

    fn word(&mut self, word: &str, value: Value) -> Result<Value, String> {
        if self.src[self.pos..].starts_with(word.as_bytes()) {
            self.pos += word.len();
            Ok(value)
        } else {
            Err(self.error("expected a value"))
        }
    }

    fn number(&mut self) -> Result<Value, String> {
        let start = self.pos;
        while self
            .peek()
            .is_some_and(|c| c.is_ascii_digit() || b"+-.eE".contains(&c))
        {
            self.pos += 1;
        }
        std::str::from_utf8(&self.src[start..self.pos])
            .ok()
            .and_then(|s| s.parse().ok())
            .map(Value::Number)
            .ok_or_else(|| self.error("invalid number"))
    }

    fn string(&mut self) -> Result<String, String> {
        self.pos += 1;
        let mut out = Vec::new();
        loop {
            let c = self
                .peek()
                .ok_or_else(|| self.error("unterminated string"))?;
            self.pos += 1;
            match c {
                b'"' => break,
                b'\\' => {
                    let e = self
                        .peek()
                        .ok_or_else(|| self.error("unterminated string"))?;
                    self.pos += 1;
                    match e {
                        b'n' => out.push(b'\n'),
                        b't' => out.push(b'\t'),
                        b'r' => out.push(b'\r'),
                        b'b' | b'f' => {}
                        b'u' => {
                            let hex = self
                                .src
                                .get(self.pos..self.pos + 4)
                                .and_then(|h| std::str::from_utf8(h).ok())
                                .and_then(|h| u32::from_str_radix(h, 16).ok())
                                .ok_or_else(|| self.error("invalid escape"))?;
                            self.pos += 4;
                            let ch = char::from_u32(hex).unwrap_or('\u{fffd}');
                            out.extend_from_slice(ch.encode_utf8(&mut [0; 4]).as_bytes());
                        }
                        other => out.push(other),
                    }
                }
                other => out.push(other),
            }
        }
        Ok(String::from_utf8_lossy(&out).into_owned())
    }

    /// Items up to `close`, separated by commas, allowing a trailing comma.
    fn items(
        &mut self,
        close: u8,
        mut item: impl FnMut(&mut Self) -> Result<(), String>,
    ) -> Result<(), String> {
        self.pos += 1;
        loop {
            self.skip_space();
            if self.peek() == Some(close) {
                self.pos += 1;
                return Ok(());
            }
            item(self)?;
            self.skip_space();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(c) if c == close => {}
                _ => return Err(self.error("expected ',' or a closing bracket")),
            }
        }
    }

    fn array(&mut self, depth: usize) -> Result<Value, String> {
        let mut out = Vec::new();
        self.items(b']', |p| {
            out.push(p.value(depth + 1)?);
            Ok(())
        })?;
        Ok(Value::Array(out))
    }

    fn object(&mut self, depth: usize) -> Result<Value, String> {
        let mut out = BTreeMap::new();
        self.items(b'}', |p| {
            if p.peek() != Some(b'"') {
                return Err(p.error("expected a key"));
            }
            let key = p.string()?;
            p.skip_space();
            if p.peek() != Some(b':') {
                return Err(p.error("expected ':'"));
            }
            p.pos += 1;
            out.insert(key, p.value(depth + 1)?);
            Ok(())
        })?;
        Ok(Value::Object(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_lenient_files() {
        let v = parse(
            "\u{feff}{\n \"name\": \"A\tB\", // comment\n \"curve\": [[0, 1.5], [1e3, -2],],\n \"ok\": true, \"none\": null,\n}",
        )
        .unwrap();
        assert_eq!(v.get("NAME").and_then(Value::as_str), Some("A\tB"));
        let curve = v.get("curve").unwrap().as_array();
        assert_eq!(curve[1].as_array()[0].as_f64(), Some(1000.0));
        assert_eq!(curve[1].as_array()[1].as_f64(), Some(-2.0));
        assert_eq!(v.get("ok"), Some(&Value::Bool(true)));
        assert!(parse("{\"a\": }").is_err());
        assert!(parse("[1, 2").is_err());
    }
}
