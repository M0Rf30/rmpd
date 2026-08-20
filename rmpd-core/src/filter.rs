//! MPD filter expression parsing and evaluation.
//!
//! Mirrors `src/song/Filter.cxx` in upstream MPD (0.21+ grammar) plus the
//! explicit case-sensitivity operators added in 0.24. MPD only supports
//! `AND` (no `OR`) at the expression level; negation always wraps a fully
//! parenthesized group.
//!
//! ```text
//! GROUP  := '(' GROUP ')'
//!         | '(' GROUP ('AND' GROUP)+ ')'
//!         | '(' '!' GROUP ')'
//!         | '(' TERM ')'
//! TERM   := ('any' | 'file' | 'filename' | TAG) STROP QUOTED
//!         | 'base' QUOTED
//!         | 'modified-since' QUOTED
//!         | 'added-since' QUOTED
//!         | 'AudioFormat' ('==' | '=~') QUOTED
//!         | 'prio' '>=' NUMBER
//! STROP  := '==' | '!=' | '=~' | '!~'
//!         | 'contains' | '!contains' | 'starts_with' | '!starts_with'
//!         | 'eq_cs' | 'eq_ci' | '!eq_cs' | '!eq_ci'
//!         | 'contains_cs' | 'contains_ci' | '!contains_cs' | '!contains_ci'
//!         | 'starts_with_cs' | 'starts_with_ci' | '!starts_with_cs' | '!starts_with_ci'
//! ```
//!
//! The legacy pre-0.21 syntax (`find TAG VALUE [TAG VALUE ...]`) is handled
//! by [`FilterExpression::from_pairs`], which mirrors `SongFilter::Parse`'s
//! per-pair dispatch (same special tag names, AND-combined).
use crate::error::{Result, RmpdError};
use crate::path::uri_safe_local;
use crate::song::canonical_tag_name;
use crate::tag::tag_fallback_chain;

#[derive(Debug, Clone, PartialEq)]
pub enum FilterExpression {
    /// Tag (or `any`/`file`) comparison.
    Compare {
        tag: String,
        op: CompareOp,
        value: String,
        case_sensitive: bool,
        negated: bool,
    },
    /// `(base "URI")`: restrict to songs under the given directory.
    Base(String),
    /// `(modified-since "TIMESTAMP")`, stored as a Unix timestamp.
    ModifiedSince(i64),
    /// `(added-since "TIMESTAMP")`, stored as a Unix timestamp.
    AddedSince(i64),
    /// `(AudioFormat == 'SAMPLERATE:BITS:CHANNELS')` (or `=~` for a mask;
    /// `None` components are wildcards).
    AudioFormat {
        sample_rate: Option<u32>,
        bits: Option<u16>,
        channels: Option<u8>,
    },
    /// `(prio >= N)`. Database songs are never queued, so this only matches
    /// when `N == 0` (MPD's default song priority).
    Priority(u8),
    And(Box<FilterExpression>, Box<FilterExpression>),
    Not(Box<FilterExpression>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompareOp {
    Equal,      // == / eq_cs / eq_ci
    Contains,   // contains / contains_cs / contains_ci
    StartsWith, // starts_with / starts_with_cs / starts_with_ci
    Regex,      // =~
}

/// Classification of a filter term's leading identifier, mirroring
/// `locate_parse_type()` in MPD's Filter.cxx. Note the mixed case rules: MPD
/// checks `file`/`filename`/`any`/`AudioFormat`/`prio` case-insensitively but
/// `base`/`modified-since`/`added-since` with exact-case `strcmp`.
enum FilterKeyword {
    File,
    Any,
    Base,
    ModifiedSince,
    AddedSince,
    AudioFormat,
    Priority,
    Tag(String),
}

fn classify_filter_keyword(name: &str) -> FilterKeyword {
    if name.eq_ignore_ascii_case("file") || name.eq_ignore_ascii_case("filename") {
        FilterKeyword::File
    } else if name.eq_ignore_ascii_case("any") {
        FilterKeyword::Any
    } else if name == "base" {
        FilterKeyword::Base
    } else if name == "modified-since" {
        FilterKeyword::ModifiedSince
    } else if name == "added-since" {
        FilterKeyword::AddedSince
    } else if name.eq_ignore_ascii_case("audioformat") {
        FilterKeyword::AudioFormat
    } else if name.eq_ignore_ascii_case("prio") {
        FilterKeyword::Priority
    } else {
        FilterKeyword::Tag(name.to_lowercase())
    }
}

fn is_known_tag(tag_lower: &str) -> bool {
    canonical_tag_name(tag_lower) != "Unknown"
}

// `uri_safe_local` (only the legacy 2-arg `base` form is checked this way;
// the modern `(base "...")` expression form accepts any value, including "")
// lives in `crate::path`, shared with `update`/`rescan`'s path validation.

/// Parse an MPD-style timestamp: ISO 8601 (`YYYY-MM-DD[THH:MM[:SS]][Z|±HHMM]`
/// or the month-only form `YYYY-MM`), falling back to a plain Unix timestamp.
/// Mirrors `ParseTimeStamp()` in MPD's Filter.cxx.
fn parse_timestamp(s: &str) -> Result<i64> {
    if let Some(ts) = parse_iso8601(s) {
        return Ok(ts);
    }
    s.trim()
        .parse::<i64>()
        .map_err(|_| RmpdError::ParseError(format!("Failed to parse timestamp: {s}")))
}

/// Days since the Unix epoch for a given proleptic-Gregorian date (Howard
/// Hinnant's `days_from_civil` algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn parse_iso8601(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.len() < 7 || s.as_bytes()[4] != b'-' {
        return None;
    }
    let year: i64 = s.get(0..4)?.parse().ok()?;
    let month: i64 = s.get(5..7)?.parse().ok()?;
    let rest = &s[7..];
    if rest.is_empty() {
        // "YYYY-MM" (month-only)
        return Some(days_from_civil(year, month, 1) * 86400);
    }
    let rest = rest.strip_prefix('-')?;
    let day: i64 = rest.get(0..2)?.parse().ok()?;
    let mut rest = &rest[2..];

    let mut secs_of_day = 0i64;
    if let Some(after_t) = rest.strip_prefix('T') {
        let hh: i64 = after_t.get(0..2)?.parse().ok()?;
        let mut r = &after_t[2..];
        let mut mm = 0i64;
        let mut ss = 0i64;
        if let Some(after_colon) = r.strip_prefix(':') {
            mm = after_colon.get(0..2)?.parse().ok()?;
            r = &after_colon[2..];
            if let Some(after_colon2) = r.strip_prefix(':') {
                ss = after_colon2.get(0..2)?.parse().ok()?;
                r = &after_colon2[2..];
            }
        }
        secs_of_day = hh * 3600 + mm * 60 + ss;
        rest = r;
    }

    let mut ts = days_from_civil(year, month, day) * 86400 + secs_of_day;
    if rest.is_empty() || rest == "Z" {
        // UTC (or unspecified, treated as UTC).
        return Some(ts);
    }
    let sign = match rest.as_bytes()[0] {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let off = &rest[1..];
    let (oh, om): (i64, i64) = if let Some((h, m)) = off.split_once(':') {
        (h.parse().ok()?, m.parse().ok()?)
    } else if off.len() == 4 {
        (off.get(0..2)?.parse().ok()?, off.get(2..4)?.parse().ok()?)
    } else {
        (off.parse().ok()?, 0)
    };
    ts -= sign * (oh * 3600 + om * 60);
    Some(ts)
}

fn parse_audio_format_component<T: std::str::FromStr>(s: &str, mask: bool) -> Result<Option<T>> {
    if s == "*" {
        if mask {
            return Ok(None);
        }
        return Err(RmpdError::ParseError(
            "'*' is only allowed with the '=~' operator".to_string(),
        ));
    }
    s.parse::<T>()
        .map(Some)
        .map_err(|_| RmpdError::ParseError(format!("Failed to parse audio format: {s}")))
}

/// Parse `SAMPLERATE:BITS:CHANNELS`, matching MPD's `ParseAudioFormat()`.
/// `*` components are only valid in mask mode (`=~`).
fn parse_audio_format(s: &str, mask: bool) -> Result<(Option<u32>, Option<u16>, Option<u8>)> {
    let mut parts = s.split(':');
    let (Some(sr), Some(bits), Some(ch), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(RmpdError::ParseError(format!(
            "Failed to parse audio format: {s}"
        )));
    };
    let sample_rate: Option<u32> = parse_audio_format_component(sr, mask)?;
    let bits: Option<u16> = parse_audio_format_component(bits, mask)?;
    let channels: Option<u8> = parse_audio_format_component(ch, mask)?;
    Ok((sample_rate, bits, channels))
}

impl FilterExpression {
    /// Parse a modern `(...)` filter expression. `fold_case` is the
    /// command's default case sensitivity (`true` for `search`-family
    /// commands, `false` for `find`-family), overridable per-term by the
    /// explicit `_cs`/`_ci` operator suffixes.
    pub fn parse(input: &str, fold_case: bool) -> Result<Self> {
        let mut parser = Parser::new(input.trim());
        let expr = parser.parse_group(fold_case)?;
        parser.skip_ws();
        if parser.pos != parser.input.len() {
            return Err(RmpdError::ParseError(
                "Unparsed garbage after expression".to_string(),
            ));
        }
        Ok(expr)
    }

    /// Build a filter from legacy `find`/`search` pairs (`TAG VALUE [TAG
    /// VALUE ...]`), AND-combined, mirroring `SongFilter::Parse(args, ...)`.
    pub fn from_pairs(pairs: &[(String, String)], fold_case: bool) -> Result<Self> {
        let mut iter = pairs.iter();
        let Some((tag0, val0)) = iter.next() else {
            return Err(RmpdError::ParseError(
                "Incorrect number of filter arguments".to_string(),
            ));
        };
        let mut expr = Self::from_pair(tag0, val0, fold_case)?;
        for (tag, val) in iter {
            expr = FilterExpression::And(
                Box::new(expr),
                Box::new(Self::from_pair(tag, val, fold_case)?),
            );
        }
        Ok(expr)
    }

    /// Mirrors `SongFilter::Parse(tag_string, value, fold_case, ...)`.
    fn from_pair(tag: &str, value: &str, fold_case: bool) -> Result<Self> {
        match classify_filter_keyword(tag) {
            FilterKeyword::Base => {
                if !uri_safe_local(value) {
                    return Err(RmpdError::ParseError("Bad URI".to_string()));
                }
                Ok(FilterExpression::Base(value.to_string()))
            }
            FilterKeyword::ModifiedSince => {
                Ok(FilterExpression::ModifiedSince(parse_timestamp(value)?))
            }
            FilterKeyword::AddedSince => Ok(FilterExpression::AddedSince(parse_timestamp(value)?)),
            FilterKeyword::File => Ok(Self::legacy_compare("file", value, fold_case)),
            FilterKeyword::Any => Ok(Self::legacy_compare("any", value, fold_case)),
            // MPD's own 2-arg `SongFilter::Parse` has no case for AudioFormat/prio:
            // they fall into the generic `default:` branch and get cast to an
            // out-of-range `TagType` — undefined behaviour in the C++
            // implementation, not a real feature. Reject as an unknown tag
            // instead of replicating that.
            FilterKeyword::AudioFormat | FilterKeyword::Priority => {
                Err(RmpdError::ParseError("Unknown filter type".to_string()))
            }
            FilterKeyword::Tag(tag_lower) => {
                if !is_known_tag(&tag_lower) {
                    return Err(RmpdError::ParseError("Unknown filter type".to_string()));
                }
                Ok(Self::legacy_compare(&tag_lower, value, fold_case))
            }
        }
    }

    /// For compatibility with MPD 0.20 and older, `fold_case` also switches
    /// on substring matching (not just case-insensitivity).
    fn legacy_compare(tag: &str, value: &str, fold_case: bool) -> Self {
        FilterExpression::Compare {
            tag: tag.to_string(),
            op: if fold_case {
                CompareOp::Contains
            } else {
                CompareOp::Equal
            },
            value: value.to_string(),
            case_sensitive: !fold_case,
            negated: false,
        }
    }

    /// Convert filter expression to SQL WHERE clause using EXISTS subqueries on song_tags.
    /// The songs table is referenced as `songs` (no alias).
    pub fn to_sql(&self) -> (String, Vec<String>) {
        match self {
            FilterExpression::Compare {
                tag,
                op,
                value,
                case_sensitive,
                negated,
            } => Self::compare_to_sql(tag, *op, value, *case_sensitive, *negated),
            FilterExpression::Base(value) => {
                if value.is_empty() {
                    // Mirrors `uri_is_child_or_same("", child)`: matches any
                    // non-empty path (i.e. every real song).
                    ("path != ''".to_string(), Vec::new())
                } else {
                    (
                        "(path = ? OR path LIKE ? ESCAPE '\\')".to_string(),
                        vec![value.clone(), format!("{}/%", escape_like_value(value))],
                    )
                }
            }
            FilterExpression::ModifiedSince(ts) => {
                ("last_modified >= ?".to_string(), vec![ts.to_string()])
            }
            FilterExpression::AddedSince(ts) => ("added_at >= ?".to_string(), vec![ts.to_string()]),
            FilterExpression::AudioFormat {
                sample_rate,
                bits,
                channels,
            } => {
                let mut conds = vec![
                    "sample_rate IS NOT NULL".to_string(),
                    "channels IS NOT NULL".to_string(),
                    "bits_per_sample IS NOT NULL".to_string(),
                ];
                let mut params = Vec::new();
                if let Some(v) = sample_rate {
                    conds.push("sample_rate = ?".to_string());
                    params.push(v.to_string());
                }
                if let Some(v) = bits {
                    conds.push("bits_per_sample = ?".to_string());
                    params.push(v.to_string());
                }
                if let Some(v) = channels {
                    conds.push("channels = ?".to_string());
                    params.push(v.to_string());
                }
                (format!("({})", conds.join(" AND ")), params)
            }
            // Database songs are never queued, so their implicit priority is
            // always 0 (MPD's default `LightSong::priority`).
            FilterExpression::Priority(n) => {
                if *n == 0 {
                    ("1".to_string(), Vec::new())
                } else {
                    ("0".to_string(), Vec::new())
                }
            }
            FilterExpression::And(left, right) => {
                let (left_sql, mut left_params) = left.to_sql();
                let (right_sql, right_params) = right.to_sql();
                left_params.extend(right_params);
                (format!("({left_sql} AND {right_sql})"), left_params)
            }
            FilterExpression::Not(expr) => {
                let (sql, params) = expr.to_sql();
                (format!("NOT ({sql})"), params)
            }
        }
    }

    fn compare_to_sql(
        tag: &str,
        op: CompareOp,
        value: &str,
        case_sensitive: bool,
        negated: bool,
    ) -> (String, Vec<String>) {
        let tag_lower = tag.to_lowercase();

        if tag_lower == "file" {
            let (cond, params) = compare_sql("path", op, value, case_sensitive);
            return if negated {
                (format!("NOT ({cond})"), params)
            } else {
                (cond, params)
            };
        }

        let any = tag_lower == "any";
        let fallback_tags: Vec<&str> = if any {
            Vec::new()
        } else {
            tag_fallback_chain(&tag_lower)
        };
        let tag_restriction = if any {
            String::new()
        } else {
            let placeholders = fallback_tags
                .iter()
                .map(|_| "?")
                .collect::<Vec<_>>()
                .join(", ");
            format!(" AND st.tag IN ({placeholders})")
        };
        let tag_params: Vec<String> = fallback_tags.iter().map(|t| t.to_string()).collect();

        // MPD treats a missing tag as an empty value, so comparing a tag (or
        // its fallback chain) against the empty string must also match songs
        // that have no row at all for those tags.
        if value.is_empty() && op == CompareOp::Equal {
            let has_nonempty = format!(
                "EXISTS (SELECT 1 FROM song_tags st WHERE st.song_id = songs.id{tag_restriction} AND st.value != '')"
            );
            return if negated {
                (has_nonempty, tag_params)
            } else {
                (format!("NOT {has_nonempty}"), tag_params)
            };
        }

        let (cmp_cond, cmp_params) = compare_sql("st.value", op, value, case_sensitive);
        let mut params = tag_params;
        params.extend(cmp_params);
        let sql = format!(
            "EXISTS (SELECT 1 FROM song_tags st WHERE st.song_id = songs.id{tag_restriction} AND {cmp_cond})"
        );
        if negated {
            (format!("NOT ({sql})"), params)
        } else {
            (sql, params)
        }
    }
}

/// Backslash-escapes SQL LIKE metacharacters (`\`, `%`, `_`) in a client-supplied
/// value so `Contains`/`StartsWith` only ever match the literal substring; the
/// paired `ESCAPE '\'` clause on the operator makes the backslash active.
fn escape_like_value(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for c in value.chars() {
        if matches!(c, '\\' | '%' | '_') {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    escaped
}

/// Build the SQL fragment for one string comparison against `column`
/// (case-insensitive variants rely on SQLite's default ASCII-only case
/// folding for `LIKE`/`COLLATE NOCASE` — matching the rest of rmpd's search
/// paths, which have the same limitation).
fn compare_sql(
    column: &str,
    op: CompareOp,
    value: &str,
    case_sensitive: bool,
) -> (String, Vec<String>) {
    match op {
        CompareOp::Equal if case_sensitive => (format!("{column} = ?"), vec![value.to_string()]),
        CompareOp::Equal => (
            format!("{column} = ? COLLATE NOCASE"),
            vec![value.to_string()],
        ),
        CompareOp::Contains if case_sensitive => {
            (format!("INSTR({column}, ?) > 0"), vec![value.to_string()])
        }
        CompareOp::Contains => (
            format!("{column} LIKE ? ESCAPE '\\'"),
            vec![format!("%{}%", escape_like_value(value))],
        ),
        CompareOp::StartsWith if case_sensitive => (
            format!("SUBSTR({column}, 1, LENGTH(?)) = ?"),
            vec![value.to_string(), value.to_string()],
        ),
        CompareOp::StartsWith => (
            format!("{column} LIKE ? ESCAPE '\\'"),
            vec![format!("{}%", escape_like_value(value))],
        ),
        CompareOp::Regex => {
            // No per-row case-sensitivity flag on the custom `regexp()`
            // SQLite function, so fold case via an inline regex flag instead.
            let pattern = if case_sensitive {
                value.to_string()
            } else {
                format!("(?i){value}")
            };
            (format!("{column} REGEXP ?"), vec![pattern])
        }
    }
}

struct Parser<'a> {
    input: &'a str,
    pos: usize,
}

impl<'a> Parser<'a> {
    fn new(input: &'a str) -> Self {
        Self { input, pos: 0 }
    }

    fn peek(&self) -> Option<char> {
        self.input[self.pos..].chars().next()
    }

    fn advance_char(&mut self) {
        if let Some(c) = self.peek() {
            self.pos += c.len_utf8();
        }
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(c) if c.is_whitespace()) {
            self.advance_char();
        }
    }

    /// Matches a literal token with no boundary requirement (for symbolic
    /// operators like `==`/`=~`, which MPD allows directly against a quote).
    fn consume_literal(&mut self, lit: &str) -> bool {
        if self.input[self.pos..].starts_with(lit) {
            self.pos += lit.len();
            true
        } else {
            false
        }
    }

    /// Matches a keyword token that must be followed by whitespace (or EOF),
    /// so e.g. `contains` doesn't spuriously match a `contains_cs` prefix.
    fn consume_word(&mut self, word: &str) -> bool {
        let rest = &self.input[self.pos..];
        if !rest.starts_with(word) {
            return false;
        }
        match rest.as_bytes().get(word.len()) {
            Some(b) if !b.is_ascii_whitespace() => false,
            _ => {
                self.pos += word.len();
                true
            }
        }
    }

    /// Reads a tag-name identifier (`IsTagNameChar`: ASCII alpha, `_`, `-`).
    fn expect_word(&mut self) -> Result<String> {
        let start = self.pos;
        while matches!(self.peek(), Some(c) if c.is_ascii_alphabetic() || c == '_' || c == '-') {
            self.advance_char();
        }
        if self.pos == start {
            return Err(RmpdError::ParseError("Word expected".to_string()));
        }
        let word = self.input[start..self.pos].to_string();
        self.skip_ws();
        Ok(word)
    }

    fn expect_and_keyword(&mut self) -> Result<()> {
        if self.expect_word()? != "AND" {
            return Err(RmpdError::ParseError("'AND' expected".to_string()));
        }
        Ok(())
    }

    fn expect_byte(&mut self, expected: char, message: &str) -> Result<()> {
        if self.peek() != Some(expected) {
            return Err(RmpdError::ParseError(message.to_string()));
        }
        self.advance_char();
        self.skip_ws();
        Ok(())
    }

    fn expect_quoted(&mut self) -> Result<String> {
        let quote = self
            .peek()
            .ok_or_else(|| RmpdError::ParseError("Quoted string expected".to_string()))?;
        if quote != '\'' && quote != '"' {
            return Err(RmpdError::ParseError("Quoted string expected".to_string()));
        }
        self.advance_char();

        let mut result = String::new();
        loop {
            let ch = self
                .peek()
                .ok_or_else(|| RmpdError::ParseError("Closing quote not found".to_string()))?;
            if ch == quote {
                self.advance_char();
                break;
            }
            if ch == '\\' {
                self.advance_char();
                let escaped = self
                    .peek()
                    .ok_or_else(|| RmpdError::ParseError("Closing quote not found".to_string()))?;
                result.push(escaped);
                self.advance_char();
            } else {
                result.push(ch);
                self.advance_char();
            }
        }
        self.skip_ws();
        Ok(result)
    }

    /// Mirrors `ParseStringFilter()`: order matters — explicit `_cs`/`_ci`
    /// variants must be tried before their bare counterparts.
    fn parse_string_operator(&mut self) -> Result<(CompareOp, Option<bool>, bool)> {
        const VARIANTS: &[(&str, CompareOp, bool, bool)] = &[
            ("contains_cs", CompareOp::Contains, true, false),
            ("!contains_cs", CompareOp::Contains, true, true),
            ("contains_ci", CompareOp::Contains, false, false),
            ("!contains_ci", CompareOp::Contains, false, true),
            ("starts_with_cs", CompareOp::StartsWith, true, false),
            ("!starts_with_cs", CompareOp::StartsWith, true, true),
            ("starts_with_ci", CompareOp::StartsWith, false, false),
            ("!starts_with_ci", CompareOp::StartsWith, false, true),
            ("eq_cs", CompareOp::Equal, true, false),
            ("!eq_cs", CompareOp::Equal, true, true),
            ("eq_ci", CompareOp::Equal, false, false),
            ("!eq_ci", CompareOp::Equal, false, true),
        ];
        for (word, op, cs, neg) in VARIANTS {
            if self.consume_word(word) {
                self.skip_ws();
                return Ok((*op, Some(*cs), *neg));
            }
        }
        if self.consume_word("!contains") {
            self.skip_ws();
            return Ok((CompareOp::Contains, None, true));
        }
        if self.consume_word("contains") {
            self.skip_ws();
            return Ok((CompareOp::Contains, None, false));
        }
        if self.consume_word("!starts_with") {
            self.skip_ws();
            return Ok((CompareOp::StartsWith, None, true));
        }
        if self.consume_word("starts_with") {
            self.skip_ws();
            return Ok((CompareOp::StartsWith, None, false));
        }
        if self.consume_literal("!~") {
            self.skip_ws();
            return Ok((CompareOp::Regex, None, true));
        }
        if self.consume_literal("=~") {
            self.skip_ws();
            return Ok((CompareOp::Regex, None, false));
        }
        if self.consume_literal("!=") {
            self.skip_ws();
            return Ok((CompareOp::Equal, None, true));
        }
        if self.consume_literal("==") {
            self.skip_ws();
            return Ok((CompareOp::Equal, None, false));
        }
        Err(RmpdError::ParseError(format!(
            "Unknown filter operator: {}",
            &self.input[self.pos..]
        )))
    }

    /// Parses one `(...)` group. `fold_case` is the command's default case
    /// sensitivity, threaded down for terms that don't specify `_cs`/`_ci`.
    fn parse_group(&mut self, fold_case: bool) -> Result<FilterExpression> {
        self.expect_byte('(', "'(' expected")?;

        if self.peek() == Some('(') {
            let mut expr = self.parse_group(fold_case)?;
            self.skip_ws();
            if self.peek() == Some(')') {
                self.advance_char();
                self.skip_ws();
                return Ok(expr);
            }
            self.expect_and_keyword()?;
            self.skip_ws();
            loop {
                let next = self.parse_group(fold_case)?;
                expr = FilterExpression::And(Box::new(expr), Box::new(next));
                self.skip_ws();
                if self.peek() == Some(')') {
                    self.advance_char();
                    self.skip_ws();
                    return Ok(expr);
                }
                self.expect_and_keyword()?;
                self.skip_ws();
            }
        }

        if self.peek() == Some('!') {
            self.advance_char();
            self.skip_ws();
            if self.peek() != Some('(') {
                return Err(RmpdError::ParseError("'(' expected".to_string()));
            }
            let inner = self.parse_group(fold_case)?;
            self.skip_ws();
            if self.peek() != Some(')') {
                return Err(RmpdError::ParseError("')' expected".to_string()));
            }
            self.advance_char();
            self.skip_ws();
            return Ok(FilterExpression::Not(Box::new(inner)));
        }

        let name = self.expect_word()?;
        let expr = self.parse_term_body(&name, fold_case)?;
        self.skip_ws();
        if self.peek() != Some(')') {
            return Err(RmpdError::ParseError("')' expected".to_string()));
        }
        self.advance_char();
        self.skip_ws();
        Ok(expr)
    }

    fn parse_term_body(&mut self, name: &str, fold_case: bool) -> Result<FilterExpression> {
        match classify_filter_keyword(name) {
            FilterKeyword::Base => Ok(FilterExpression::Base(self.expect_quoted()?)),
            FilterKeyword::ModifiedSince => Ok(FilterExpression::ModifiedSince(parse_timestamp(
                &self.expect_quoted()?,
            )?)),
            FilterKeyword::AddedSince => Ok(FilterExpression::AddedSince(parse_timestamp(
                &self.expect_quoted()?,
            )?)),
            FilterKeyword::AudioFormat => {
                let mask = if self.consume_literal("==") {
                    false
                } else if self.consume_literal("=~") {
                    true
                } else {
                    return Err(RmpdError::ParseError("'==' or '=~' expected".to_string()));
                };
                self.skip_ws();
                let value = self.expect_quoted()?;
                let (sample_rate, bits, channels) = parse_audio_format(&value, mask)?;
                Ok(FilterExpression::AudioFormat {
                    sample_rate,
                    bits,
                    channels,
                })
            }
            FilterKeyword::Priority => {
                if !self.consume_literal(">=") {
                    return Err(RmpdError::ParseError("'>=' expected".to_string()));
                }
                self.skip_ws();
                let start = self.pos;
                while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                    self.advance_char();
                }
                if self.pos == start {
                    return Err(RmpdError::ParseError("Number expected".to_string()));
                }
                let value: u32 = self.input[start..self.pos]
                    .parse()
                    .map_err(|_| RmpdError::ParseError("Number expected".to_string()))?;
                if value > 0xff {
                    return Err(RmpdError::ParseError("Invalid priority value".to_string()));
                }
                self.skip_ws();
                Ok(FilterExpression::Priority(value as u8))
            }
            FilterKeyword::File => self.parse_compare_tail("file", fold_case),
            FilterKeyword::Any => self.parse_compare_tail("any", fold_case),
            FilterKeyword::Tag(tag_lower) => {
                if !is_known_tag(&tag_lower) {
                    return Err(RmpdError::ParseError(format!(
                        "Unknown filter type: {name}"
                    )));
                }
                self.parse_compare_tail(&tag_lower, fold_case)
            }
        }
    }

    fn parse_compare_tail(&mut self, tag: &str, fold_case: bool) -> Result<FilterExpression> {
        let (op, case_sensitive_override, negated) = self.parse_string_operator()?;
        let value = self.expect_quoted()?;
        Ok(FilterExpression::Compare {
            tag: tag.to_string(),
            op,
            value,
            case_sensitive: case_sensitive_override.unwrap_or(!fold_case),
            negated,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(input: &str, fold_case: bool) -> FilterExpression {
        FilterExpression::parse(input, fold_case).unwrap()
    }

    #[test]
    fn test_simple_comparison() {
        let expr = parse("(Artist == 'Radiohead')", false);
        let (sql, params) = expr.to_sql();
        assert!(sql.contains("song_tags"));
        assert_eq!(params, vec!["artist", "Radiohead"]);
    }

    #[test]
    fn test_and_expression() {
        let expr = parse("((date == '2000') AND (genre == 'Rock'))", false);
        let (sql, params) = expr.to_sql();
        assert!(sql.contains("AND"), "SQL should contain AND: {sql}");
        assert_eq!(params, vec!["date", "2000", "genre", "Rock"]);
    }

    #[test]
    fn test_three_way_and_chain() {
        // MPD's AND-chain is variadic, not just binary.
        let expr = parse(
            "((artist == '1') AND (album == '2') AND (genre == '3'))",
            false,
        );
        let (sql, _) = expr.to_sql();
        // 2 structural ANDs from the chain itself, plus 2 per comparison
        // (one from the tag_restriction's own `AND`, one joining it to the
        // value comparison) — 3 comparisons here.
        assert_eq!(sql.matches("AND").count(), 8);
    }

    #[test]
    fn test_or_is_not_supported() {
        // MPD's grammar has no OR at all.
        assert!(FilterExpression::parse("((artist == 'A') OR (artist == 'B'))", false).is_err());
    }

    #[test]
    fn test_not_expression() {
        let expr = parse("(!(genre == 'Pop'))", false);
        let (sql, _) = expr.to_sql();
        assert!(sql.starts_with("NOT ("));
    }

    #[test]
    fn test_not_requires_parens() {
        assert!(FilterExpression::parse("(!genre == 'Pop')", false).is_err());
    }

    #[test]
    fn test_regex() {
        let expr = parse("(Artist =~ 'Radio.*')", false);
        let (sql, params) = expr.to_sql();
        assert!(sql.contains("REGEXP"), "SQL should contain REGEXP: {sql}");
        assert!(!sql.contains("LIKE"), "SQL must not contain LIKE: {sql}");
        assert_eq!(params, vec!["artist", "Radio.*"]);
    }

    #[test]
    fn test_not_regex() {
        let expr = parse("(Artist !~ 'Radio.*')", false);
        let (sql, _) = expr.to_sql();
        assert!(sql.starts_with("NOT (") && sql.contains("REGEXP"));
    }

    #[test]
    fn test_double_quoted_values() {
        let expr = parse("(Artist == \"Amon Tobin\")", false);
        let (sql, params) = expr.to_sql();
        assert!(sql.contains("song_tags"));
        assert_eq!(params, vec!["artist", "Amon Tobin"]);
    }

    #[test]
    fn test_double_quoted_with_escape() {
        let expr = parse(r#"(Artist == "Guns \"N\" Roses")"#, false);
        let (_, params) = expr.to_sql();
        assert_eq!(params, vec!["artist", r#"Guns "N" Roses"#]);
    }

    #[test]
    fn test_albumartist_fallback() {
        let expr = parse("(AlbumArtist == 'Led Zeppelin')", false);
        let (sql, params) = expr.to_sql();
        assert!(sql.contains("IN"));
        assert_eq!(params, vec!["albumartist", "artist", "Led Zeppelin"]);
    }

    #[test]
    fn test_file_tag() {
        let expr = parse("(file == 'some/path.mp3')", false);
        let (sql, params) = expr.to_sql();
        assert_eq!(sql, "path = ?");
        assert_eq!(params, vec!["some/path.mp3"]);
    }

    #[test]
    fn test_any_tag_has_no_tag_restriction() {
        let expr = parse("(any contains 'foo')", true);
        let (sql, params) = expr.to_sql();
        assert!(
            !sql.contains("st.tag IN"),
            "any must not restrict tag: {sql}"
        );
        assert_eq!(params, vec!["%foo%"]);
    }

    #[test]
    fn test_empty_equality_matches_missing_tag() {
        let expr = parse("(AlbumArtist == '')", false);
        let (sql, params) = expr.to_sql();
        assert!(
            sql.starts_with("NOT EXISTS"),
            "expected NOT EXISTS, got: {sql}"
        );
        assert!(sql.contains("st.value != ''"), "got: {sql}");
        assert_eq!(params, vec!["albumartist", "artist"]);
    }

    #[test]
    fn test_empty_inequality_matches_present_value() {
        let expr = parse("(Artist != '')", false);
        let (sql, params) = expr.to_sql();
        assert!(sql.starts_with("EXISTS"), "expected EXISTS, got: {sql}");
        assert!(sql.contains("st.value != ''"), "got: {sql}");
        assert_eq!(params, vec!["artist"]);
    }

    #[test]
    fn test_find_default_is_case_sensitive_equal() {
        let expr = parse("(Artist == 'X')", false);
        let (sql, _) = expr.to_sql();
        assert!(
            !sql.contains("COLLATE NOCASE"),
            "find should be case-sensitive: {sql}"
        );
    }

    #[test]
    fn test_search_default_is_case_insensitive_equal() {
        let expr = parse("(Artist == 'X')", true);
        let (sql, _) = expr.to_sql();
        assert!(
            sql.contains("COLLATE NOCASE"),
            "search should be case-insensitive: {sql}"
        );
    }

    #[test]
    fn test_explicit_case_sensitivity_overrides_command_default() {
        // eq_cs under `search` (fold_case=true) stays case-sensitive.
        let expr = parse("(Artist eq_cs 'X')", true);
        let (sql, _) = expr.to_sql();
        assert!(
            !sql.contains("COLLATE NOCASE"),
            "eq_cs must override search's default: {sql}"
        );

        // eq_ci under `find` (fold_case=false) stays case-insensitive.
        let expr = parse("(Artist eq_ci 'X')", false);
        let (sql, _) = expr.to_sql();
        assert!(
            sql.contains("COLLATE NOCASE"),
            "eq_ci must override find's default: {sql}"
        );
    }

    #[test]
    fn test_case_sensitive_contains_uses_instr_not_like() {
        let expr = parse("(Artist contains_cs 'X')", true);
        let (sql, params) = expr.to_sql();
        assert!(sql.contains("INSTR"), "got: {sql}");
        assert_eq!(params, vec!["artist", "X"]);
    }

    #[test]
    fn test_negated_contains() {
        // Bare `!contains` (no `_cs`/`_ci` suffix) follows the command's
        // default case sensitivity; use search semantics (fold_case=true)
        // to exercise the LIKE-based case-insensitive path.
        let expr = parse("(Artist !contains 'X')", true);
        let (sql, _) = expr.to_sql();
        assert!(sql.starts_with("NOT (") && sql.contains("LIKE"));
    }

    #[test]
    fn test_negated_starts_with_ci() {
        let expr = parse("(Artist !starts_with_ci 'X')", false);
        let (sql, _) = expr.to_sql();
        assert!(sql.starts_with("NOT (") && sql.contains("LIKE"));
    }

    #[test]
    fn test_base_filter() {
        let expr = parse("(base 'Music/Rock')", false);
        let (sql, params) = expr.to_sql();
        assert_eq!(sql, "(path = ? OR path LIKE ? ESCAPE '\\')");
        assert_eq!(params, vec!["Music/Rock", "Music/Rock/%"]);
    }

    #[test]
    fn test_base_empty_matches_everything() {
        let expr = parse("(base '')", false);
        let (sql, params) = expr.to_sql();
        assert_eq!(sql, "path != ''");
        assert!(params.is_empty());
    }

    #[test]
    fn test_modified_since_iso8601() {
        let expr = parse("(modified-since '2023-01-15T10:30:00Z')", false);
        let (sql, params) = expr.to_sql();
        assert_eq!(sql, "last_modified >= ?");
        // 2023-01-15T10:30:00Z
        assert_eq!(params, vec!["1673778600"]);
    }

    #[test]
    fn test_modified_since_unix_timestamp() {
        let expr = parse("(modified-since '1700000000')", false);
        let (_, params) = expr.to_sql();
        assert_eq!(params, vec!["1700000000"]);
    }

    #[test]
    fn test_added_since() {
        let expr = parse("(added-since '2023-01-15')", false);
        let (sql, params) = expr.to_sql();
        assert_eq!(sql, "added_at >= ?");
        assert_eq!(params, vec!["1673740800"]);
    }

    #[test]
    fn test_audio_format_exact() {
        let expr = parse("(AudioFormat == '44100:16:2')", false);
        let (sql, params) = expr.to_sql();
        assert!(sql.contains("sample_rate = ?"));
        assert!(sql.contains("bits_per_sample = ?"));
        assert!(sql.contains("channels = ?"));
        assert_eq!(params, vec!["44100", "16", "2"]);
    }

    #[test]
    fn test_audio_format_mask() {
        let expr = parse("(AudioFormat =~ '*:16:2')", false);
        let (sql, params) = expr.to_sql();
        assert!(!sql.contains("sample_rate = ?"));
        assert!(sql.contains("bits_per_sample = ?"));
        assert_eq!(params, vec!["16", "2"]);
    }

    #[test]
    fn test_audio_format_wildcard_rejected_without_mask() {
        assert!(FilterExpression::parse("(AudioFormat == '*:16:2')", false).is_err());
    }

    #[test]
    fn test_priority_filter() {
        let expr = parse("(prio >= 0)", false);
        let (sql, params) = expr.to_sql();
        assert_eq!(sql, "1");
        assert!(params.is_empty());

        let expr = parse("(prio >= 5)", false);
        let (sql, _) = expr.to_sql();
        assert_eq!(sql, "0");
    }

    #[test]
    fn test_priority_out_of_range() {
        assert!(FilterExpression::parse("(prio >= 256)", false).is_err());
    }

    #[test]
    fn test_unknown_tag_error_message() {
        let err = FilterExpression::parse("(notarealtag == 'x')", false).unwrap_err();
        assert_eq!(
            err.to_string(),
            "Parse error: Unknown filter type: notarealtag"
        );
    }

    #[test]
    fn test_unknown_operator_error_message() {
        let err = FilterExpression::parse("(Artist <> 'x')", false).unwrap_err();
        assert!(
            err.to_string().contains("Unknown filter operator"),
            "got: {err}"
        );
    }

    #[test]
    fn test_unparsed_garbage_after_expression() {
        let err = FilterExpression::parse("(Artist == 'x') garbage", false).unwrap_err();
        assert!(
            err.to_string()
                .contains("Unparsed garbage after expression")
        );
    }

    #[test]
    fn test_legacy_pairs_find_is_equal_case_sensitive() {
        let expr =
            FilterExpression::from_pairs(&[("artist".to_string(), "Radiohead".to_string())], false)
                .unwrap();
        let (sql, params) = expr.to_sql();
        assert!(!sql.contains("LIKE") && !sql.contains("COLLATE"));
        assert_eq!(params, vec!["artist", "Radiohead"]);
    }

    #[test]
    fn test_legacy_pairs_search_is_contains_case_insensitive() {
        let expr =
            FilterExpression::from_pairs(&[("artist".to_string(), "radio".to_string())], true)
                .unwrap();
        let (sql, params) = expr.to_sql();
        assert!(sql.contains("LIKE"));
        assert_eq!(params, vec!["artist", "%radio%"]);
    }

    #[test]
    fn test_legacy_pairs_multi_and() {
        let expr = FilterExpression::from_pairs(
            &[
                ("artist".to_string(), "A".to_string()),
                ("album".to_string(), "B".to_string()),
            ],
            false,
        )
        .unwrap();
        let (sql, params) = expr.to_sql();
        assert!(sql.contains(" AND "));
        assert_eq!(params, vec!["artist", "A", "album", "B"]);
    }

    #[test]
    fn test_legacy_pairs_unknown_tag_rejected() {
        // e.g. a `sort`/`window` keyword leaking into filter pairs for a
        // command that doesn't strip them (count/searchcount).
        let err = FilterExpression::from_pairs(&[("sort".to_string(), "Album".to_string())], false)
            .unwrap_err();
        assert_eq!(err.to_string(), "Parse error: Unknown filter type");
    }

    #[test]
    fn test_legacy_pairs_base() {
        let expr =
            FilterExpression::from_pairs(&[("base".to_string(), "Music/Rock".to_string())], false)
                .unwrap();
        assert_eq!(expr, FilterExpression::Base("Music/Rock".to_string()));
    }

    #[test]
    fn test_legacy_pairs_base_unsafe_uri_rejected() {
        let err =
            FilterExpression::from_pairs(&[("base".to_string(), "../etc".to_string())], false)
                .unwrap_err();
        assert_eq!(err.to_string(), "Parse error: Bad URI");
    }

    #[test]
    fn test_legacy_pairs_empty_rejected() {
        assert!(FilterExpression::from_pairs(&[], false).is_err());
    }
}
