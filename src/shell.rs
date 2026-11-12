//! A small, forgiving parser and printer for "mongo shell" style documents.
//!
//! The printer and the parser round-trip every BSON type, so opening a document in the
//! editor and saving it untouched never changes a field's type:
//!   Int32  -> 42            Int64 -> NumberLong(42)      Double -> 42.0
//!   Date   -> ISODate("…")  ObjectId -> ObjectId("…")    Decimal128 -> NumberDecimal("…")

use anyhow::{Result, anyhow, bail};
use mongodb::bson::{self, Bson, Document, oid::ObjectId, spec::BinarySubtype};
use std::fmt::Write as _;

// ---------------------------------------------------------------- printing

pub fn format_double(d: f64) -> String {
    if d.is_nan() {
        return "NaN".into();
    }
    if d.is_infinite() {
        return if d > 0.0 { "Infinity".into() } else { "-Infinity".into() };
    }
    if d.fract() == 0.0 && d.abs() < 1e15 {
        return format!("{d:.1}");
    }
    let s = format!("{d:?}");
    if s.contains('.') || s.contains('e') || s.contains('E') { s } else { format!("{s}.0") }
}

pub fn quote(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

fn is_plain_key(k: &str) -> bool {
    let mut chars = k.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' || c == '$' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
}

pub fn format_key(k: &str) -> String {
    if is_plain_key(k) { k.to_string() } else { quote(k) }
}

/// One-line rendering of a scalar. Documents and arrays are handled by the callers.
pub fn format_scalar(v: &Bson) -> String {
    match v {
        Bson::Double(d) => format_double(*d),
        Bson::String(s) => quote(s),
        Bson::Boolean(b) => b.to_string(),
        Bson::Null => "null".into(),
        Bson::Int32(i) => i.to_string(),
        Bson::Int64(i) => {
            if i.unsigned_abs() > (1u64 << 53) {
                format!("NumberLong(\"{i}\")")
            } else {
                format!("NumberLong({i})")
            }
        }
        Bson::ObjectId(o) => format!("ObjectId(\"{}\")", o.to_hex()),
        Bson::DateTime(d) => match d.try_to_rfc3339_string() {
            Ok(s) => format!("ISODate(\"{s}\")"),
            Err(_) => format!("new Date({})", d.timestamp_millis()),
        },
        Bson::Decimal128(d) => format!("NumberDecimal(\"{d}\")"),
        Bson::Timestamp(t) => format!("Timestamp({}, {})", t.time, t.increment),
        Bson::RegularExpression(r) => format!("RegExp({}, {})", quote(&r.pattern), quote(&r.options)),
        Bson::Binary(b) => {
            if b.subtype == BinarySubtype::Uuid && b.bytes.len() == 16 {
                let h = hex::encode(&b.bytes);
                format!("UUID(\"{}-{}-{}-{}-{}\")", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
            } else {
                use base64::Engine;
                let b64 = base64::engine::general_purpose::STANDARD.encode(&b.bytes);
                format!("BinData({}, \"{}\")", u8::from(b.subtype), b64)
            }
        }
        Bson::MinKey => "MinKey()".into(),
        Bson::MaxKey => "MaxKey()".into(),
        Bson::Undefined => "undefined".into(),
        Bson::JavaScriptCode(c) => format!("Code({})", quote(c)),
        Bson::Symbol(s) => format!("{{\"$symbol\": {}}}", quote(s)),
        // Very rare legacy types: fall back to canonical extended JSON, which the parser accepts.
        other => other.clone().into_canonical_extjson().to_string(),
    }
}

pub fn doc_to_shell(d: &Document) -> String {
    let mut out = String::new();
    write_doc(&mut out, d, 2, 0);
    out
}

fn pad(out: &mut String, n: usize) {
    for _ in 0..n {
        out.push(' ');
    }
}

fn write_doc(out: &mut String, d: &Document, indent: usize, level: usize) {
    if d.is_empty() {
        out.push_str("{}");
        return;
    }
    out.push_str("{\n");
    let n = d.len();
    for (i, (k, v)) in d.iter().enumerate() {
        pad(out, indent * (level + 1));
        let _ = write!(out, "{}: ", format_key(k));
        write_value(out, v, indent, level + 1);
        if i + 1 < n {
            out.push(',');
        }
        out.push('\n');
    }
    pad(out, indent * level);
    out.push('}');
}

fn write_value(out: &mut String, v: &Bson, indent: usize, level: usize) {
    match v {
        Bson::Document(d) => write_doc(out, d, indent, level),
        Bson::Array(a) => {
            if a.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push_str("[\n");
            for (i, item) in a.iter().enumerate() {
                pad(out, indent * (level + 1));
                write_value(out, item, indent, level + 1);
                if i + 1 < a.len() {
                    out.push(',');
                }
                out.push('\n');
            }
            pad(out, indent * level);
            out.push(']');
        }
        other => out.push_str(&format_scalar(other)),
    }
}

// ---------------------------------------------------------------- parsing

/// Parse a filter / sort / document. An empty input means an empty document.
pub fn parse_document(src: &str) -> Result<Document> {
    if src.trim().is_empty() {
        return Ok(Document::new());
    }
    match parse_value(src)? {
        Bson::Document(d) => Ok(d),
        _ => bail!("Expected a document like {{ field: 'value' }}"),
    }
}

pub fn parse_value(src: &str) -> Result<Bson> {
    let mut p = Parser { s: src.as_bytes(), src, i: 0 };
    p.ws();
    let v = p.value()?;
    p.ws();
    if p.i < p.s.len() {
        return Err(p.err("Unexpected text after the end of the value"));
    }
    Ok(v)
}

struct Parser<'a> {
    s: &'a [u8],
    src: &'a str,
    i: usize,
}

const EXTJSON_KEYS: &[&str] = &[
    "$oid", "$date", "$numberLong", "$numberInt", "$numberDouble", "$numberDecimal", "$binary",
    "$uuid", "$timestamp", "$regularExpression", "$minKey", "$maxKey", "$undefined", "$symbol",
    "$code", "$dbPointer",
];

impl<'a> Parser<'a> {
    fn err(&self, msg: &str) -> anyhow::Error {
        let upto = &self.src[..self.i.min(self.src.len())];
        let line = upto.matches('\n').count() + 1;
        let col = upto.rsplit('\n').next().map(|l| l.chars().count()).unwrap_or(0) + 1;
        anyhow!("{msg} (line {line}, column {col})")
    }

    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }

    fn ws(&mut self) {
        loop {
            match self.peek() {
                Some(c) if c.is_ascii_whitespace() => self.i += 1,
                Some(b'/') if self.s.get(self.i + 1) == Some(&b'/') => {
                    while let Some(c) = self.peek() {
                        if c == b'\n' {
                            break;
                        }
                        self.i += 1;
                    }
                }
                _ => break,
            }
        }
    }

    fn eat(&mut self, c: u8) -> bool {
        self.ws();
        if self.peek() == Some(c) {
            self.i += 1;
            true
        } else {
            false
        }
    }

    fn expect(&mut self, c: u8) -> Result<()> {
        if self.eat(c) { Ok(()) } else { Err(self.err(&format!("Expected '{}'", c as char))) }
    }

    fn value(&mut self) -> Result<Bson> {
        self.ws();
        match self.peek() {
            None => Err(self.err("Unexpected end of input")),
            Some(b'{') => self.object(),
            Some(b'[') => self.array(),
            Some(b'"') | Some(b'\'') => Ok(Bson::String(self.string()?)),
            Some(b'/') => self.regex_literal(),
            Some(c) if c == b'-' || c == b'+' || c == b'.' || c.is_ascii_digit() => self.number(),
            Some(c) if c.is_ascii_alphabetic() || c == b'_' || c == b'$' => self.ident_value(),
            Some(c) => Err(self.err(&format!("Unexpected character '{}'", c as char))),
        }
    }

    fn object(&mut self) -> Result<Bson> {
        self.expect(b'{')?;
        let mut doc = Document::new();
        loop {
            self.ws();
            if self.eat(b'}') {
                break;
            }
            let key = match self.peek() {
                Some(b'"') | Some(b'\'') => self.string()?,
                Some(c) if c.is_ascii_alphanumeric() || c == b'_' || c == b'$' => {
                    let start = self.i;
                    while let Some(c) = self.peek() {
                        if c.is_ascii_alphanumeric() || c == b'_' || c == b'$' || c == b'.' {
                            self.i += 1;
                        } else {
                            break;
                        }
                    }
                    self.src[start..self.i].to_string()
                }
                _ => return Err(self.err("Expected a field name")),
            };
            self.expect(b':')?;
            let v = self.value()?;
            doc.insert(key, v);
            self.ws();
            if self.eat(b',') {
                continue;
            }
            self.expect(b'}')?;
            break;
        }
        // Extended JSON wrappers such as {"$oid": "…"} become real BSON types.
        if let Some(first) = doc.keys().next() {
            if EXTJSON_KEYS.contains(&first.as_str()) {
                let json = Bson::Document(doc.clone()).into_canonical_extjson();
                if let Ok(b) = Bson::try_from(json) {
                    return Ok(b);
                }
            }
        }
        Ok(Bson::Document(doc))
    }

    fn array(&mut self) -> Result<Bson> {
        self.expect(b'[')?;
        let mut items = Vec::new();
        loop {
            self.ws();
            if self.eat(b']') {
                break;
            }
            items.push(self.value()?);
            self.ws();
            if self.eat(b',') {
                continue;
            }
            self.expect(b']')?;
            break;
        }
        Ok(Bson::Array(items))
    }

    fn string(&mut self) -> Result<String> {
        self.ws();
        let q = self.peek().ok_or_else(|| self.err("Expected a string"))?;
        if q != b'"' && q != b'\'' {
            return Err(self.err("Expected a quoted string"));
        }
        self.i += 1;
        let mut out = String::new();
        loop {
            let rest = &self.src[self.i..];
            let mut chars = rest.chars();
            let c = chars.next().ok_or_else(|| self.err("Unterminated string"))?;
            self.i += c.len_utf8();
            if c as u32 == q as u32 {
                break;
            }
            if c != '\\' {
                out.push(c);
                continue;
            }
            let e = chars.next().ok_or_else(|| self.err("Unterminated string"))?;
            self.i += e.len_utf8();
            match e {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'r' => out.push('\r'),
                'b' => out.push('\u{8}'),
                'f' => out.push('\u{c}'),
                '0' => out.push('\0'),
                'u' => {
                    let cp = self.hex4()?;
                    if (0xD800..0xDC00).contains(&cp) && self.src[self.i..].starts_with("\\u") {
                        self.i += 2;
                        let lo = self.hex4()?;
                        let full = 0x10000 + ((cp - 0xD800) << 10) + (lo.wrapping_sub(0xDC00) & 0x3FF);
                        out.push(char::from_u32(full).unwrap_or('\u{FFFD}'));
                    } else {
                        out.push(char::from_u32(cp).unwrap_or('\u{FFFD}'));
                    }
                }
                other => out.push(other),
            }
        }
        Ok(out)
    }

    fn hex4(&mut self) -> Result<u32> {
        let h = self.src.get(self.i..self.i + 4).ok_or_else(|| self.err("Bad \\u escape"))?;
        let cp = u32::from_str_radix(h, 16).map_err(|_| self.err("Bad \\u escape"))?;
        self.i += 4;
        Ok(cp)
    }

    fn number(&mut self) -> Result<Bson> {
        let start = self.i;
        if matches!(self.peek(), Some(b'-') | Some(b'+')) {
            self.i += 1;
        }
        if self.src[self.i..].starts_with("Infinity") {
            self.i += 8;
            let neg = self.src[start..].starts_with('-');
            return Ok(Bson::Double(if neg { f64::NEG_INFINITY } else { f64::INFINITY }));
        }
        let mut is_float = false;
        while let Some(c) = self.peek() {
            match c {
                b'0'..=b'9' => self.i += 1,
                b'.' => {
                    is_float = true;
                    self.i += 1;
                }
                b'e' | b'E' => {
                    is_float = true;
                    self.i += 1;
                    if matches!(self.peek(), Some(b'-') | Some(b'+')) {
                        self.i += 1;
                    }
                }
                _ => break,
            }
        }
        let text = self.src[start..self.i].trim_start_matches('+');
        if !is_float {
            if let Ok(i) = text.parse::<i32>() {
                return Ok(Bson::Int32(i));
            }
            if let Ok(i) = text.parse::<i64>() {
                return Ok(Bson::Int64(i));
            }
        }
        text.parse::<f64>().map(Bson::Double).map_err(|_| self.err("Invalid number"))
    }

    fn regex_literal(&mut self) -> Result<Bson> {
        self.i += 1; // opening slash
        let start = self.i;
        let mut escaped = false;
        loop {
            let c = self.peek().ok_or_else(|| self.err("Unterminated /regex/"))?;
            if c == b'/' && !escaped {
                break;
            }
            escaped = c == b'\\' && !escaped;
            self.i += 1;
        }
        let pattern = self.src[start..self.i].to_string();
        self.i += 1;
        let fstart = self.i;
        while matches!(self.peek(), Some(c) if c.is_ascii_alphabetic()) {
            self.i += 1;
        }
        let options = self.src[fstart..self.i].to_string();
        Ok(Bson::RegularExpression(bson::Regex { pattern, options }))
    }

    fn ident(&mut self) -> String {
        let start = self.i;
        while matches!(self.peek(), Some(c) if c.is_ascii_alphanumeric() || c == b'_' || c == b'$') {
            self.i += 1;
        }
        self.src[start..self.i].to_string()
    }

    fn args(&mut self) -> Result<Vec<Bson>> {
        let mut args = Vec::new();
        if !self.eat(b'(') {
            return Ok(args);
        }
        loop {
            self.ws();
            if self.eat(b')') {
                break;
            }
            args.push(self.value()?);
            self.ws();
            if self.eat(b',') {
                continue;
            }
            self.expect(b')')?;
            break;
        }
        Ok(args)
    }

    fn ident_value(&mut self) -> Result<Bson> {
        let mut name = self.ident();
        if name == "new" {
            self.ws();
            name = self.ident();
        }
        match name.as_str() {
            "true" => return Ok(Bson::Boolean(true)),
            "false" => return Ok(Bson::Boolean(false)),
            "null" => return Ok(Bson::Null),
            "undefined" => return Ok(Bson::Undefined),
            "NaN" => return Ok(Bson::Double(f64::NAN)),
            "Infinity" => return Ok(Bson::Double(f64::INFINITY)),
            _ => {}
        }
        let args = self.args()?;
        let str_arg = |n: usize| -> Option<String> {
            match args.get(n) {
                Some(Bson::String(s)) => Some(s.clone()),
                Some(Bson::Int32(i)) => Some(i.to_string()),
                Some(Bson::Int64(i)) => Some(i.to_string()),
                Some(Bson::Double(d)) => Some(d.to_string()),
                _ => None,
            }
        };
        let num_arg = |n: usize| -> Option<i64> {
            match args.get(n) {
                Some(Bson::Int32(i)) => Some(*i as i64),
                Some(Bson::Int64(i)) => Some(*i),
                Some(Bson::Double(d)) => Some(*d as i64),
                Some(Bson::String(s)) => s.trim().parse().ok(),
                _ => None,
            }
        };
        match name.as_str() {
            "ObjectId" => match str_arg(0) {
                None => Ok(Bson::ObjectId(ObjectId::new())),
                Some(h) => ObjectId::parse_str(h.trim())
                    .map(Bson::ObjectId)
                    .map_err(|_| self.err("ObjectId needs 24 hex characters")),
            },
            "ISODate" | "Date" => match args.first() {
                None => Ok(Bson::DateTime(bson::DateTime::now())),
                Some(Bson::String(s)) => parse_date(s).map(Bson::DateTime).ok_or_else(|| {
                    self.err("Could not read the date. Use e.g. ISODate(\"2026-01-31T13:45:00Z\")")
                }),
                Some(_) => num_arg(0)
                    .map(|ms| Bson::DateTime(bson::DateTime::from_millis(ms)))
                    .ok_or_else(|| self.err("Date needs a string or milliseconds")),
            },
            "NumberLong" | "Long" => {
                num_arg(0).map(Bson::Int64).ok_or_else(|| self.err("NumberLong needs a whole number"))
            }
            "NumberInt" | "Int32" => num_arg(0)
                .and_then(|n| i32::try_from(n).ok())
                .map(Bson::Int32)
                .ok_or_else(|| self.err("NumberInt needs a 32-bit whole number")),
            "NumberDouble" | "Double" => str_arg(0)
                .and_then(|s| s.trim().parse::<f64>().ok())
                .map(Bson::Double)
                .ok_or_else(|| self.err("Double needs a number")),
            "NumberDecimal" | "Decimal128" => str_arg(0)
                .and_then(|s| s.trim().parse::<bson::Decimal128>().ok())
                .map(Bson::Decimal128)
                .ok_or_else(|| self.err("NumberDecimal needs a decimal number")),
            "UUID" => {
                let s = str_arg(0).unwrap_or_default().replace('-', "");
                let bytes = hex::decode(s.trim()).map_err(|_| self.err("UUID needs 32 hex characters"))?;
                if bytes.len() != 16 {
                    return Err(self.err("UUID needs 32 hex characters"));
                }
                Ok(Bson::Binary(bson::Binary { subtype: BinarySubtype::Uuid, bytes }))
            }
            "BinData" => {
                use base64::Engine;
                let sub = num_arg(0).ok_or_else(|| self.err("BinData(subtype, \"base64\")"))? as u8;
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(str_arg(1).unwrap_or_default().trim())
                    .map_err(|_| self.err("BinData needs valid base64"))?;
                Ok(Bson::Binary(bson::Binary { subtype: BinarySubtype::from(sub), bytes }))
            }
            "Timestamp" => {
                let (t, i) = match args.first() {
                    Some(Bson::Document(d)) => (
                        d.get("t").and_then(bson_i64).unwrap_or(0),
                        d.get("i").and_then(bson_i64).unwrap_or(0),
                    ),
                    _ => (num_arg(0).unwrap_or(0), num_arg(1).unwrap_or(0)),
                };
                Ok(Bson::Timestamp(bson::Timestamp { time: t as u32, increment: i as u32 }))
            }
            "RegExp" => Ok(Bson::RegularExpression(bson::Regex {
                pattern: str_arg(0).unwrap_or_default(),
                options: str_arg(1).unwrap_or_default(),
            })),
            "MinKey" => Ok(Bson::MinKey),
            "MaxKey" => Ok(Bson::MaxKey),
            "Code" => Ok(Bson::JavaScriptCode(str_arg(0).unwrap_or_default())),
            other => Err(self.err(&format!(
                "Unknown word '{other}'. Text values need quotes, e.g. 'value'"
            ))),
        }
    }
}

fn bson_i64(b: &Bson) -> Option<i64> {
    match b {
        Bson::Int32(i) => Some(*i as i64),
        Bson::Int64(i) => Some(*i),
        Bson::Double(d) => Some(*d as i64),
        _ => None,
    }
}

fn parse_date(s: &str) -> Option<bson::DateTime> {
    let s = s.trim();
    let candidates = [
        s.to_string(),
        format!("{s}Z"),
        format!("{s}:00Z"),
        format!("{s}T00:00:00Z"),
        s.replacen(' ', "T", 1),
        format!("{}Z", s.replacen(' ', "T", 1)),
    ];
    candidates.iter().find_map(|c| bson::DateTime::parse_rfc3339_str(c).ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mongodb::bson::doc;

    #[test]
    fn parses_shell_style_filter() {
        let d = parse_document(
            "{ name: 'x', n: {$gte: 5}, 'a.b': \"q\", id: ObjectId('65f0c0ffee65f0c0ffee65f0'), \
             when: {$lt: ISODate('2026-10-05')}, tags: {$in: [1, 2.5, NumberLong(7)]}, r: /^ab\\/c/i, }",
        )
        .unwrap();
        assert_eq!(d.get_str("name").unwrap(), "x");
        assert_eq!(d.get_document("n").unwrap(), &doc! {"$gte": 5});
        assert!(matches!(d.get("id"), Some(Bson::ObjectId(_))));
        assert!(matches!(d.get_document("when").unwrap().get("$lt"), Some(Bson::DateTime(_))));
        assert_eq!(
            d.get_document("tags").unwrap().get_array("$in").unwrap(),
            &vec![Bson::Int32(1), Bson::Double(2.5), Bson::Int64(7)]
        );
        match d.get("r") {
            Some(Bson::RegularExpression(r)) => {
                assert_eq!(r.pattern, "^ab\\/c");
                assert_eq!(r.options, "i");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn round_trips_types() {
        let original = doc! {
            "_id": ObjectId::parse_str("65f0c0ffee65f0c0ffee65f0").unwrap(),
            "i32": 100,
            "i64": 100i64,
            "big": 9007199254740993i64,
            "dbl": 100.0,
            "frac": 1.5,
            "tiny": 1e-7,
            "huge": 1e300,
            "s": "he said \"hi\"\n\u{1F600}",
            "b": true,
            "n": Bson::Null,
            "d": bson::DateTime::from_millis(1_790_000_000_245),
            "dec": "12.3400".parse::<bson::Decimal128>().unwrap(),
            "ts": bson::Timestamp { time: 5, increment: 6 },
            "re": bson::Regex { pattern: "a/b".into(), options: "i".into() },
            "bin": bson::Binary { subtype: BinarySubtype::Generic, bytes: vec![1, 2, 3] },
            "uuid": bson::Binary { subtype: BinarySubtype::Uuid, bytes: (0u8..16).collect() },
            "min": Bson::MinKey,
            "max": Bson::MaxKey,
            "undef": Bson::Undefined,
            "code": Bson::JavaScriptCode("x=1".into()),
            "sym": Bson::Symbol("s".into()),
            "weird key": {"nested": [1, {"a": []}, {}]},
        };
        let text = doc_to_shell(&original);
        let back = parse_document(&text).unwrap_or_else(|e| panic!("{e}\n{text}"));
        assert_eq!(original, back, "\n{text}");
    }

    #[test]
    fn reads_extended_json() {
        let d = parse_document(r#"{"_id": {"$oid": "65f0c0ffee65f0c0ffee65f0"}, "n": {"$numberLong": "5"}}"#).unwrap();
        assert!(matches!(d.get("_id"), Some(Bson::ObjectId(_))));
        assert_eq!(d.get("n"), Some(&Bson::Int64(5)));
    }

    #[test]
    fn reports_position() {
        let e = parse_document("{ a: 1,\n b: oops }").unwrap_err().to_string();
        assert!(e.contains("line 2"), "{e}");
    }
}
