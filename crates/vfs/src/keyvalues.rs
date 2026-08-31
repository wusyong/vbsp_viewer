//! Valve's KeyValues 1 text format.
//!
//! One parser serves three jobs: `gameinfo.txt`, every `.vmt` material, and the
//! BSP `ENTITIES` lump. They are the same grammar, so they get the same code.
//!
//! ```text
//! "GameInfo"
//! {
//!     game "Team Fortress 2"      // a comment
//!     FileSystem { SearchPaths { game+mod tf/custom/* } }
//! }
//! ```
//!
//! # What the format actually allows
//!
//! - Tokens are either double-quoted (and may then contain spaces, `{`, `}`
//!   and `//`) or bare, ending at whitespace or a brace.
//! - Keys repeat. `hidden_maps` in gameinfo and `[$LDR]`/`[$HDR]` pairs in
//!   materials both rely on it, so entries are an **ordered list**, not a map,
//!   and [`KeyValues::get`] returns the first match the way Valve's own reader
//!   does.
//! - Keys are matched **case-insensitively**: maps write `$BaseTexture`,
//!   `$basetexture` and `$Basetexture` interchangeably.
//! - `//` runs to end of line. There are no block comments.
//! - A bare token in `[brackets]` after a value is a platform/HDR condition.
//!   Phase 1 keeps the value and drops the condition — see [`Parser::parse`].
//! - No escape sequences. `\` is an ordinary character, which matters because
//!   Windows paths appear in these files verbatim.
//!
//! The entity lump is a *sequence* of top-level blocks rather than one root, so
//! [`KeyValues::parse`] returns a document that may hold several — worldspawn is
//! simply the first `entity` block.

use std::fmt;

#[derive(Debug, thiserror::Error)]
pub enum KvError {
    #[error("line {line}: unexpected `}}` with no open block")]
    UnbalancedClose { line: usize },

    #[error("line {line}: unterminated quoted string")]
    UnterminatedQuote { line: usize },

    #[error("line {line}: {} unclosed block(s) at end of input", count)]
    UnclosedBlock { line: usize, count: usize },

    #[error("line {line}: expected a key, found `{{`")]
    BlockWithoutKey { line: usize },
}

pub type Result<T> = std::result::Result<T, KvError>;

/// A key's value: either a leaf string or a nested block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    String(String),
    Block(KeyValues),
}

impl Value {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            Value::Block(_) => None,
        }
    }

    pub fn as_block(&self) -> Option<&KeyValues> {
        match self {
            Value::Block(kv) => Some(kv),
            Value::String(_) => None,
        }
    }
}

/// An ordered list of key/value entries. See the module docs for why this is
/// not a `HashMap`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KeyValues {
    entries: Vec<(String, Value)>,
}

impl KeyValues {
    pub fn new() -> Self {
        Self::default()
    }

    /// Parse a whole document, which may contain more than one top-level block.
    pub fn parse(text: &str) -> Result<KeyValues> {
        Parser::new(text).parse()
    }

    /// Parse a bare sequence of `{ ... }` blocks with no keys — the shape of
    /// the BSP `ENTITIES` lump, where each block is one entity.
    pub fn parse_blocks(text: &str) -> Result<Vec<KeyValues>> {
        Parser::new(text).parse_blocks()
    }

    pub fn entries(&self) -> &[(String, Value)] {
        &self.entries
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn push(&mut self, key: impl Into<String>, value: Value) {
        self.entries.push((key.into(), value));
    }

    /// First value for `key`, case-insensitively.
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.entries
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v)
    }

    /// Every value for `key`, in file order.
    pub fn get_all<'a>(&'a self, key: &'a str) -> impl Iterator<Item = &'a Value> + 'a {
        self.entries
            .iter()
            .filter(move |(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v)
    }

    pub fn string(&self, key: &str) -> Option<&str> {
        self.get(key)?.as_str()
    }

    pub fn block(&self, key: &str) -> Option<&KeyValues> {
        self.get(key)?.as_block()
    }

    /// Follow a chain of nested block names, e.g.
    /// `path(["FileSystem", "SearchPaths"])`.
    pub fn path<'a>(&'a self, keys: impl IntoIterator<Item = &'a str>) -> Option<&'a KeyValues> {
        let mut current = self;
        for key in keys {
            current = current.block(key)?;
        }
        Some(current)
    }

    /// Parse a value as a float, tolerating the trailing junk that hand-edited
    /// materials collect.
    pub fn float(&self, key: &str) -> Option<f32> {
        let text = self.string(key)?.trim();
        text.parse().ok().or_else(|| {
            // "0.5abc" or "1 " — take the longest numeric prefix.
            let end = text
                .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == '+'))
                .unwrap_or(text.len());
            text[..end].parse().ok()
        })
    }

    /// Source treats any non-zero number as true; `$translucent "1"`.
    pub fn bool(&self, key: &str) -> Option<bool> {
        Some(self.float(key)? != 0.0)
    }

    /// Overwrite the first entry for `key`, or append if absent. This is what
    /// a VMT `Patch`'s `replace` block does.
    pub fn set(&mut self, key: &str, value: Value) {
        match self
            .entries
            .iter_mut()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
        {
            Some(slot) => slot.1 = value,
            None => self.entries.push((key.to_string(), value)),
        }
    }

    /// Append `key` only if it is not already present — VMT `insert`.
    pub fn insert_if_absent(&mut self, key: &str, value: Value) {
        if self.get(key).is_none() {
            self.entries.push((key.to_string(), value));
        }
    }
}

impl fmt::Display for KeyValues {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write(f, self, 0)
    }
}

fn write(f: &mut fmt::Formatter<'_>, kv: &KeyValues, depth: usize) -> fmt::Result {
    let pad = "  ".repeat(depth);
    for (key, value) in &kv.entries {
        match value {
            Value::String(s) => writeln!(f, "{pad}\"{key}\" \"{s}\"")?,
            Value::Block(block) => {
                writeln!(f, "{pad}\"{key}\"")?;
                writeln!(f, "{pad}{{")?;
                write(f, block, depth + 1)?;
                writeln!(f, "{pad}}}")?;
            }
        }
    }
    Ok(())
}

/// One token from the lexer, with just enough context for error messages.
#[derive(Debug, PartialEq, Eq)]
enum Token<'a> {
    /// A quoted or bare token. `quoted` matters only for conditions: a bare
    /// `[$X360]` is a condition, a quoted `"[$X360]"` is a value.
    Text { text: &'a str, quoted: bool },
    Open,
    Close,
}

struct Parser<'a> {
    src: &'a str,
    pos: usize,
    line: usize,
}

impl<'a> Parser<'a> {
    fn new(src: &'a str) -> Self {
        // A UTF-8 BOM is common in files touched by Windows editors and would
        // otherwise become part of the first key.
        let src = src.strip_prefix('\u{feff}').unwrap_or(src);
        Self { src, pos: 0, line: 1 }
    }

    /// Documents are a sequence of `key value` pairs; the value is a block if
    /// the next token is `{`.
    fn parse(&mut self) -> Result<KeyValues> {
        let (kv, depth) = self.parse_block(0)?;
        if depth != 0 {
            return Err(KvError::UnclosedBlock {
                line: self.line,
                count: depth,
            });
        }
        Ok(kv)
    }

    /// A series of keyless `{ ... }` blocks. Anything between them other than
    /// whitespace and comments is a malformed lump.
    fn parse_blocks(&mut self) -> Result<Vec<KeyValues>> {
        let mut blocks = Vec::new();
        loop {
            match self.next_token()? {
                None => return Ok(blocks),
                Some(Token::Open) => {
                    let (block, remaining) = self.parse_block(1)?;
                    if remaining != 0 {
                        return Err(KvError::UnclosedBlock {
                            line: self.line,
                            count: remaining,
                        });
                    }
                    blocks.push(block);
                }
                Some(Token::Close) => return Err(KvError::UnbalancedClose { line: self.line }),
                Some(Token::Text { .. }) => {
                    return Err(KvError::BlockWithoutKey { line: self.line })
                }
            }
        }
    }

    /// Parse entries until `}` or end of input. Returns how many blocks are
    /// still open, so the top level can report an unbalanced file.
    fn parse_block(&mut self, depth: usize) -> Result<(KeyValues, usize)> {
        let mut kv = KeyValues::new();
        loop {
            let Some(token) = self.next_token()? else {
                return Ok((kv, depth));
            };
            let key = match token {
                Token::Close if depth > 0 => return Ok((kv, depth - 1)),
                Token::Close => return Err(KvError::UnbalancedClose { line: self.line }),
                Token::Open => return Err(KvError::BlockWithoutKey { line: self.line }),
                Token::Text { text, .. } => text.to_string(),
            };

            // Peek: a `{` makes this a block, anything else is a leaf value.
            let save = (self.pos, self.line);
            match self.next_token()? {
                Some(Token::Open) => {
                    let (block, remaining) = self.parse_block(depth + 1)?;
                    if remaining != depth {
                        return Err(KvError::UnclosedBlock {
                            line: self.line,
                            count: remaining.saturating_sub(depth),
                        });
                    }
                    kv.push(key, Value::Block(block));
                }
                Some(Token::Text { text, quoted }) => {
                    // A bare `[...]` is a platform or HDR condition attached to
                    // the *previous* pair, not a value of its own. Phase 1
                    // keeps every variant and lets `get` take the first, which
                    // is the LDR/PC one in every material checked.
                    if !quoted && text.starts_with('[') {
                        kv.push(key, Value::String(String::new()));
                    } else {
                        let value = text.to_string();
                        // Swallow a condition that follows a real value.
                        let save = (self.pos, self.line);
                        match self.next_token()? {
                            Some(Token::Text { text, quoted: false }) if text.starts_with('[') => {}
                            _ => (self.pos, self.line) = save,
                        }
                        kv.push(key, Value::String(value));
                    }
                }
                // A key with no value at end of input, or a `}` right after:
                // `hidden_maps` style. Keep the key so callers can see it.
                Some(Token::Close) => {
                    kv.push(key, Value::String(String::new()));
                    (self.pos, self.line) = save;
                }
                None => {
                    kv.push(key, Value::String(String::new()));
                    return Ok((kv, depth));
                }
            }
        }
    }

    fn next_token(&mut self) -> Result<Option<Token<'a>>> {
        self.skip_trivia();
        let rest = &self.src[self.pos..];
        let mut chars = rest.char_indices();
        let Some((_, first)) = chars.next() else {
            return Ok(None);
        };

        match first {
            '{' => {
                self.pos += 1;
                Ok(Some(Token::Open))
            }
            '}' => {
                self.pos += 1;
                Ok(Some(Token::Close))
            }
            '"' => {
                // No escapes in KV1, so the string ends at the next quote. A
                // newline inside one means the file is malformed.
                let body = &rest[1..];
                let end = body
                    .find('"')
                    .ok_or(KvError::UnterminatedQuote { line: self.line })?;
                let text = &body[..end];
                self.line += text.matches('\n').count();
                self.pos += 1 + end + 1;
                Ok(Some(Token::Text { text, quoted: true }))
            }
            _ => {
                let end = rest
                    .find(|c: char| c.is_whitespace() || c == '{' || c == '}' || c == '"')
                    .unwrap_or(rest.len());
                let text = &rest[..end];
                self.pos += end;
                Ok(Some(Token::Text { text, quoted: false }))
            }
        }
    }

    fn skip_trivia(&mut self) {
        loop {
            let rest = &self.src[self.pos..];
            let trimmed = rest.trim_start();
            self.line += rest[..rest.len() - trimmed.len()].matches('\n').count();
            self.pos = self.src.len() - trimmed.len();

            if trimmed.starts_with("//") {
                let end = trimmed.find('\n').unwrap_or(trimmed.len());
                self.pos += end;
                continue;
            }
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &Value) -> &str {
        v.as_str().expect("string value")
    }

    #[test]
    fn parses_nested_blocks_and_comments() {
        let kv = KeyValues::parse(
            r#"
            "GameInfo"
            {
                game "Team Fortress 2"   // trailing comment
                // whole-line comment
                FileSystem
                {
                    SteamAppId 440
                }
            }
            "#,
        )
        .expect("parse");

        let info = kv.block("gameinfo").expect("root block, case-insensitive");
        assert_eq!(info.string("GAME"), Some("Team Fortress 2"));
        assert_eq!(
            info.path(["filesystem"]).and_then(|f| f.string("steamappid")),
            Some("440")
        );
    }

    #[test]
    fn brace_on_the_same_line_as_the_key() {
        // Materials and gameinfo both do this; a line-oriented parser breaks.
        let kv = KeyValues::parse(r#"Water { $refracttexture "x" }"#).expect("parse");
        assert_eq!(
            kv.block("water").and_then(|w| w.string("$refracttexture")),
            Some("x")
        );
    }

    #[test]
    fn duplicate_keys_are_all_kept_in_order() {
        let kv = KeyValues::parse(r#"a "1" a "2" a "3""#).expect("parse");
        assert_eq!(kv.string("a"), Some("1"), "get returns the first, like Valve");
        let all: Vec<&str> = kv.get_all("a").map(s).collect();
        assert_eq!(all, ["1", "2", "3"]);
    }

    #[test]
    fn quoted_values_keep_spaces_braces_and_slashes() {
        let kv = KeyValues::parse(
            r#""$basetexture" "models/player/{spy}/spy // red"
               "path" "c:\temp\x""#,
        )
        .expect("parse");
        assert_eq!(
            kv.string("$basetexture"),
            Some("models/player/{spy}/spy // red")
        );
        // No escape processing: a backslash is data, which is why Windows
        // paths in gameinfo survive.
        assert_eq!(kv.string("path"), Some(r"c:\temp\x"));
    }

    #[test]
    fn bare_tokens_end_at_braces_and_whitespace() {
        let kv = KeyValues::parse("game+mod+custom_mod\ttf/custom/*\nnodegraph 0").expect("parse");
        assert_eq!(kv.string("game+mod+custom_mod"), Some("tf/custom/*"));
        assert_eq!(kv.string("nodegraph"), Some("0"));
    }

    #[test]
    fn platform_conditions_are_dropped_but_the_value_survives() {
        // `[$WIN32]` must not become the value, and must not become a key.
        let kv = KeyValues::parse(
            r#""$basetexture" "a" [$WIN32]
               "$other" "b""#,
        )
        .expect("parse");
        assert_eq!(kv.string("$basetexture"), Some("a"));
        assert_eq!(kv.string("$other"), Some("b"));
        assert_eq!(kv.len(), 2, "the condition became an entry: {kv:?}");
    }

    #[test]
    fn a_key_with_no_value_before_a_close_brace_is_kept() {
        // gameinfo's `hidden_maps` and stray flags in materials.
        let kv = KeyValues::parse("outer { flag }").expect("parse");
        let outer = kv.block("outer").expect("block");
        assert_eq!(outer.string("flag"), Some(""));
    }

    #[test]
    fn entity_lump_is_a_sequence_of_keyless_blocks() {
        let text = r#"{ "classname" "worldspawn" "skyname" "sky_tf2_04" }
                      { "classname" "info_player_teamspawn" "origin" "0 1 2" }"#;
        let blocks = KeyValues::parse_blocks(text).expect("parse_blocks");
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].string("classname"), Some("worldspawn"));
        assert_eq!(blocks[1].string("origin"), Some("0 1 2"));

        // The same text through `parse` is an error, because a keyed document
        // and a bare sequence are genuinely different shapes.
        assert!(matches!(
            KeyValues::parse(text),
            Err(KvError::BlockWithoutKey { .. })
        ));
    }

    #[test]
    fn unbalanced_input_is_an_error_not_a_panic() {
        assert!(KeyValues::parse("a { b \"1\"").is_err(), "missing close");
        assert!(KeyValues::parse("a \"1\" }").is_err(), "extra close");
        assert!(KeyValues::parse("\"unterminated").is_err(), "open quote");
    }

    #[test]
    fn a_utf8_bom_does_not_become_part_of_the_first_key() {
        let kv = KeyValues::parse("\u{feff}\"LightmappedGeneric\" { }").expect("parse");
        assert!(kv.block("lightmappedgeneric").is_some(), "{kv:?}");
    }

    #[test]
    fn numbers_and_bools() {
        let kv = KeyValues::parse(r#"a "0.5" b "1" c "0" d "2.5abc""#).expect("parse");
        assert_eq!(kv.float("a"), Some(0.5));
        assert_eq!(kv.bool("b"), Some(true));
        assert_eq!(kv.bool("c"), Some(false));
        assert_eq!(kv.float("d"), Some(2.5), "numeric prefix of junk");
        assert_eq!(kv.float("missing"), None);
    }

    #[test]
    fn set_and_insert_match_vmt_patch_semantics() {
        let mut kv = KeyValues::parse(r#"a "1" b "2""#).expect("parse");
        kv.set("A", Value::String("9".into()));
        assert_eq!(kv.string("a"), Some("9"), "replace is case-insensitive");
        kv.set("c", Value::String("3".into()));
        assert_eq!(kv.string("c"), Some("3"), "replace appends when absent");
        kv.insert_if_absent("b", Value::String("8".into()));
        assert_eq!(kv.string("b"), Some("2"), "insert must not overwrite");
    }
}
