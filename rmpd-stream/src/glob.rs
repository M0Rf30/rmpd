// SPDX-FileCopyrightText: 2026 Gianluca Boiano
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Minimal `fnmatch(3)`-style glob matching (no `FNM_PATHNAME`: `*` also
//! matches `/`), used for `[stream].metadata_blacklist`.
//!
//! Supported syntax: `*` (any run, incl. empty), `?` (one char),
//! `[abc]` / `[a-z]` / `[!abc]` / `[^abc]` (character classes) and `\x`
//! (literal `x`). Matching is case-sensitive and operates on `char`s.

/// Result of matching a bracket expression against one character.
enum Class {
    /// Matched; the pattern continues at this index.
    Hit(usize),
    /// Well-formed class that does not match the character.
    Miss,
    /// No closing `]`: the `[` is treated as a literal.
    Malformed,
}

/// Match `p[pi] == '['` (a bracket expression) against `ch`.
fn match_class(p: &[char], pi: usize, ch: char) -> Class {
    let mut i = pi + 1;
    let negate = matches!(p.get(i), Some('!' | '^'));
    if negate {
        i += 1;
    }
    let mut matched = false;
    let mut first = true;
    loop {
        let Some(&c) = p.get(i) else {
            return Class::Malformed;
        };
        if c == ']' && !first {
            break;
        }
        first = false;
        if p.get(i + 1) == Some(&'-') && p.get(i + 2).is_some_and(|&hi| hi != ']') {
            let hi = p[i + 2];
            if (c..=hi).contains(&ch) {
                matched = true;
            }
            i += 3;
        } else {
            if c == ch {
                matched = true;
            }
            i += 1;
        }
    }
    if matched != negate {
        Class::Hit(i + 1)
    } else {
        Class::Miss
    }
}

/// Try to match the single (non-`*`) pattern element at `p[pi]` against `ch`.
/// Returns the index of the next pattern element on success.
fn match_one(p: &[char], pi: usize, ch: char) -> Option<usize> {
    match p[pi] {
        '?' => Some(pi + 1),
        '\\' if pi + 1 < p.len() => (p[pi + 1] == ch).then_some(pi + 2),
        '[' => match match_class(p, pi, ch) {
            Class::Hit(next) => Some(next),
            Class::Miss => None,
            Class::Malformed => (ch == '[').then_some(pi + 1),
        },
        c => (c == ch).then_some(pi + 1),
    }
}

/// Whether `text` matches the whole of `pattern`.
#[must_use]
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0usize, 0usize);
    // Pattern/text positions to resume from after the most recent `*`.
    let mut star: Option<(usize, usize)> = None;
    loop {
        if pi < p.len() && p[pi] == '*' {
            star = Some((pi + 1, ti));
            pi += 1;
            continue;
        }
        if ti < t.len() {
            if pi < p.len()
                && let Some(next) = match_one(&p, pi, t[ti])
            {
                pi = next;
                ti += 1;
                continue;
            }
        } else if pi >= p.len() {
            return true;
        }
        // Mismatch: let the last `*` swallow one more character.
        match star {
            Some((sp, st)) if st < t.len() => {
                star = Some((sp, st + 1));
                pi = sp;
                ti = st + 1;
            }
            _ => return false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literals_and_wildcards() {
        assert!(glob_match("abc", "abc"));
        assert!(!glob_match("abc", "abd"));
        assert!(!glob_match("abc", "abcd"));
        assert!(glob_match("", ""));
        assert!(!glob_match("", "x"));
        assert!(glob_match("*", ""));
        assert!(glob_match("*", "anything/at/all"));
        assert!(glob_match("a?c", "abc"));
        assert!(!glob_match("a?c", "ac"));
        assert!(glob_match("a*c", "ac"));
        assert!(glob_match("a*c", "abbbc"));
        assert!(!glob_match("a*c", "abbbd"));
        assert!(glob_match("*b*", "abc"));
        assert!(glob_match("a**b", "ab"));
    }

    #[test]
    fn url_patterns() {
        assert!(glob_match(
            "*://ads.example.com/*",
            "http://ads.example.com/live.mp3"
        ));
        assert!(glob_match(
            "http://*.radio.net/*",
            "http://s1.radio.net/a/b?x=1"
        ));
        assert!(!glob_match(
            "http://*.radio.net/*",
            "https://s1.radio.net/a"
        ));
        assert!(glob_match("*", "http://x"));
        assert!(!glob_match("http://exact/", "http://exact/x"));
    }

    #[test]
    fn character_classes() {
        assert!(glob_match("[abc]x", "bx"));
        assert!(!glob_match("[abc]x", "dx"));
        assert!(glob_match("[a-c]x", "bx"));
        assert!(!glob_match("[a-c]x", "dx"));
        assert!(glob_match("[!a-c]x", "dx"));
        assert!(!glob_match("[!a-c]x", "bx"));
        assert!(glob_match("[^a]x", "bx"));
        assert!(glob_match("s[0-9][0-9].example", "s42.example"));
        assert!(!glob_match("s[0-9][0-9].example", "s4.example"));
        // `]` first in the class is a literal member.
        assert!(glob_match("[]]", "]"));
        // Trailing `-` is a literal.
        assert!(glob_match("[a-]", "-"));
    }

    #[test]
    fn escapes_and_malformed() {
        assert!(glob_match(r"a\*b", "a*b"));
        assert!(!glob_match(r"a\*b", "aXb"));
        assert!(glob_match(r"a\?", "a?"));
        // Unterminated class: `[` is literal.
        assert!(glob_match("a[b", "a[b"));
        assert!(!glob_match("a[b", "ab"));
        // Trailing backslash is literal.
        assert!(glob_match(r"a\", r"a\"));
    }

    #[test]
    fn case_sensitive() {
        assert!(!glob_match("ABC", "abc"));
    }

    #[test]
    fn unicode_chars() {
        assert!(glob_match("caf?", "café"));
        assert!(glob_match("*é", "café"));
    }
}
