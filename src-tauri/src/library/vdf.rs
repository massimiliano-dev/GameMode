//! Minimal parser for Valve's KeyValues text format (`.vdf` / `.acf`).

use anyhow::{bail, Result};

#[derive(Debug, Clone, PartialEq)]
pub enum Vdf {
    Str(String),
    Obj(Vec<(String, Vdf)>),
}

impl Vdf {
    /// Case-insensitive lookup, as Steam itself treats keys.
    pub fn get(&self, key: &str) -> Option<&Vdf> {
        match self {
            Vdf::Obj(entries) => entries
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(key))
                .map(|(_, v)| v),
            Vdf::Str(_) => None,
        }
    }

    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(Vdf::as_str)
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Vdf::Str(s) => Some(s),
            Vdf::Obj(_) => None,
        }
    }

    pub fn entries(&self) -> &[(String, Vdf)] {
        match self {
            Vdf::Obj(entries) => entries,
            Vdf::Str(_) => &[],
        }
    }

    /// The object at `key`, added when missing. `None` when `self` or the
    /// existing value isn't an object.
    pub fn obj_mut(&mut self, key: &str) -> Option<&mut Vdf> {
        let Vdf::Obj(entries) = self else { return None };
        let i = match entries.iter().position(|(k, _)| k.eq_ignore_ascii_case(key)) {
            Some(i) => i,
            None => {
                entries.push((key.to_string(), Vdf::Obj(Vec::new())));
                entries.len() - 1
            }
        };
        match &mut entries[i].1 {
            obj @ Vdf::Obj(_) => Some(obj),
            Vdf::Str(_) => None,
        }
    }

    /// Sets a string value; returns whether anything changed.
    pub fn set(&mut self, key: &str, value: &str) -> bool {
        let Vdf::Obj(entries) = self else { return false };
        match entries.iter_mut().find(|(k, _)| k.eq_ignore_ascii_case(key)) {
            Some((_, Vdf::Str(old))) if old == value => false,
            Some((_, slot)) => {
                *slot = Vdf::Str(value.to_string());
                true
            }
            None => {
                entries.push((key.to_string(), Vdf::Str(value.to_string())));
                true
            }
        }
    }
}

/// Writes `root`'s entries the way Steam does: quoted, tab-indented.
pub fn write(root: &Vdf) -> String {
    fn quote(s: &str) -> String {
        format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n").replace('\t', "\\t"))
    }
    fn entries(out: &mut String, items: &[(String, Vdf)], depth: usize) {
        let indent = "\t".repeat(depth);
        for (key, value) in items {
            match value {
                Vdf::Str(s) => out.push_str(&format!("{indent}{}\t\t{}\n", quote(key), quote(s))),
                Vdf::Obj(inner) => {
                    out.push_str(&format!("{indent}{}\n{indent}{{\n", quote(key)));
                    entries(out, inner, depth + 1);
                    out.push_str(&format!("{indent}}}\n"));
                }
            }
        }
    }
    let mut out = String::new();
    entries(&mut out, root.entries(), 0);
    out
}

#[derive(Debug, PartialEq)]
enum Token {
    Str(String),
    Open,
    Close,
}

fn tokenize(src: &str) -> Result<Vec<Token>> {
    let mut tokens = Vec::new();
    let mut chars = src.chars().peekable();
    while let Some(&c) = chars.peek() {
        match c {
            c if c.is_whitespace() => {
                chars.next();
            }
            '/' => {
                // `//` line comment
                chars.next();
                if chars.peek() == Some(&'/') {
                    for c in chars.by_ref() {
                        if c == '\n' {
                            break;
                        }
                    }
                }
            }
            '{' => {
                chars.next();
                tokens.push(Token::Open);
            }
            '}' => {
                chars.next();
                tokens.push(Token::Close);
            }
            '"' => {
                chars.next();
                let mut s = String::new();
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some('n') => s.push('\n'),
                            Some('t') => s.push('\t'),
                            Some(other) => s.push(other),
                            None => bail!("unterminated escape"),
                        },
                        Some(other) => s.push(other),
                        None => bail!("unterminated string"),
                    }
                }
                tokens.push(Token::Str(s));
            }
            _ => {
                let mut s = String::new();
                while let Some(&c) = chars.peek() {
                    if c.is_whitespace() || c == '{' || c == '}' || c == '"' {
                        break;
                    }
                    s.push(c);
                    chars.next();
                }
                // Platform conditionals like `[$WIN32]` carry no data we need.
                if !(s.starts_with('[') && s.ends_with(']')) {
                    tokens.push(Token::Str(s));
                }
            }
        }
    }
    Ok(tokens)
}

pub fn parse(src: &str) -> Result<Vdf> {
    let tokens = tokenize(src)?;
    let mut pos = 0;
    let root = parse_entries(&tokens, &mut pos, false)?;
    Ok(Vdf::Obj(root))
}

fn parse_entries(tokens: &[Token], pos: &mut usize, nested: bool) -> Result<Vec<(String, Vdf)>> {
    let mut entries = Vec::new();
    while *pos < tokens.len() {
        let key = match &tokens[*pos] {
            Token::Close if nested => {
                *pos += 1;
                return Ok(entries);
            }
            Token::Str(s) => s.clone(),
            other => bail!("unexpected token {other:?}"),
        };
        *pos += 1;
        let value = match tokens.get(*pos) {
            Some(Token::Str(s)) => {
                *pos += 1;
                Vdf::Str(s.clone())
            }
            Some(Token::Open) => {
                *pos += 1;
                Vdf::Obj(parse_entries(tokens, pos, true)?)
            }
            _ => bail!("missing value for key {key:?}"),
        };
        entries.push((key, value));
    }
    if nested {
        bail!("unterminated object");
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nested_objects_escapes_and_comments() {
        let src = r#"
            // comment
            "libraryfolders"
            {
                "0"
                {
                    "path"      "C:\\Program Files (x86)\\Steam"
                    "apps" { "570" "123" }
                }
                Unquoted value [$WIN32]
            }
        "#;
        let root = parse(src).unwrap();
        let lf = root.get("LibraryFolders").unwrap();
        let first = lf.get("0").unwrap();
        assert_eq!(first.get_str("path"), Some(r"C:\Program Files (x86)\Steam"));
        assert_eq!(first.get("apps").unwrap().get_str("570"), Some("123"));
        assert_eq!(lf.get_str("Unquoted"), Some("value"));
    }

    #[test]
    fn writes_what_it_reads_and_edits_in_place() {
        let src = "\"Store\"\n{\n\t\"path\"\t\t\"C:\\\\Steam\"\n\t\"quote\"\t\t\"a \\\"b\\\"\"\n\t\"empty\"\n\t{\n\t}\n}\n";
        let mut root = parse(src).unwrap();
        assert_eq!(write(&root), src);
        let store = root.obj_mut("store").unwrap();
        assert!(store.set("PATH", r"C:\Steam") == false);
        assert!(store.obj_mut("system").unwrap().set("EnableGameOverlay", "0"));
        assert!(store.obj_mut("path").is_none(), "a string isn't an object");
        let again = parse(&write(&root)).unwrap();
        assert_eq!(again.get("Store").unwrap().get("system").unwrap().get_str("EnableGameOverlay"), Some("0"));
        assert_eq!(again.get("Store").unwrap().get_str("quote"), Some("a \"b\""));
    }

    #[test]
    fn rejects_unterminated_objects() {
        assert!(parse(r#""a" { "b" "c" "#).is_err());
    }
}
