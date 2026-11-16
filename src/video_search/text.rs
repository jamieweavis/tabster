//! Text folding and tokenising for matching song titles against video titles.

use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

/// Lowercase, strip accents, `&` → "and", drop apostrophes (so "don't" is one word) and turn every
/// other non-alphanumeric character into a space, collapsing runs of spaces.
pub(crate) fn fold(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut space = true;
    for c in s.nfkd().filter(|c| !is_combining_mark(*c)) {
        if matches!(c, '\'' | '\u{2018}' | '\u{2019}' | '`' | '\u{b4}') {
            continue;
        }
        if c == '&' {
            if !space {
                out.push(' ');
            }
            out.push_str("and ");
            space = true;
        } else if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
            space = false;
        } else if !space {
            out.push(' ');
            space = true;
        }
    }
    if out.ends_with(' ') {
        out.pop();
    }
    out
}

/// The words of [`fold`]ed text.
pub(crate) fn tokens(s: &str) -> Vec<String> {
    fold(s)
        .split(' ')
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect()
}

/// [`fold`]ed with the spaces removed, for comparing names written differently ("AC/DC", "ACDC").
pub(crate) fn compact(s: &str) -> String {
    fold(s).replace(' ', "")
}

/// Words that carry little meaning in a title, dropped from the words a match must find (unless
/// a title is made of nothing else).
pub(crate) fn is_stop_word(word: &str) -> bool {
    matches!(
        word,
        "the" | "a" | "an" | "and" | "of" | "to" | "in" | "on" | "at"
    )
}

/// How alike two words are, 0–1. Exact matches score 1; short words and numbers must match
/// exactly; longer words allow a typo or two (edit distance relative to length).
pub(crate) fn word_similarity(a: &str, b: &str) -> f64 {
    if a == b {
        return 1.0;
    }
    let (la, lb) = (a.chars().count(), b.chars().count());
    if la < 4
        || lb < 4
        || a.chars().any(|c| c.is_ascii_digit())
        || b.chars().any(|c| c.is_ascii_digit())
    {
        return 0.0;
    }
    let distance = edit_distance(a, b);
    let similarity = 1.0 - distance as f64 / la.max(lb) as f64;
    if similarity >= 0.75 { similarity } else { 0.0 }
}

/// Edit distance counting a swap of neighbouring letters as one edit (optimal string alignment).
fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    // Three rows: two back (for swaps), previous, current.
    let mut before: Vec<usize> = vec![0; b.len() + 1];
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    let mut current: Vec<usize> = vec![0; b.len() + 1];
    for i in 1..=a.len() {
        current[0] = i;
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            current[j] = (previous[j] + 1)
                .min(current[j - 1] + 1)
                .min(previous[j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                current[j] = current[j].min(before[j - 2] + 1);
            }
        }
        std::mem::swap(&mut before, &mut previous);
        std::mem::swap(&mut previous, &mut current);
    }
    previous[b.len()]
}

/// Whether `needle` appears in `haystack` as consecutive words.
pub(crate) fn contains_phrase(haystack: &[String], needle: &[&str]) -> bool {
    !needle.is_empty()
        && haystack.len() >= needle.len()
        && haystack
            .windows(needle.len())
            .any(|w| w.iter().zip(needle).all(|(a, b)| a == b))
}

/// The parts of a title outside brackets, and the bracketed parts' contents.
/// "Song (Live) [HD]" → ("Song ", ["Live", "HD"]). Unbalanced brackets are treated as text.
pub(crate) fn split_brackets(s: &str) -> (String, Vec<String>) {
    let mut outside = String::new();
    let mut inside = Vec::new();
    let mut depth = 0usize;
    let mut current = String::new();
    for c in s.chars() {
        match c {
            '(' | '[' | '{' => {
                if depth == 0 {
                    current.clear();
                } else {
                    current.push(c);
                }
                depth += 1;
            }
            ')' | ']' | '}' if depth > 0 => {
                depth -= 1;
                if depth == 0 {
                    inside.push(std::mem::take(&mut current));
                    outside.push(' ');
                } else {
                    current.push(c);
                }
            }
            _ if depth > 0 => current.push(c),
            _ => outside.push(c),
        }
    }
    if depth > 0 {
        // An unclosed bracket: keep its text as ordinary text.
        outside.push(' ');
        outside.push_str(&current);
    }
    (outside, inside)
}

/// Cut a "feat. X" / "ft. X" / "featuring X" clause (and everything after it) off folded words.
pub(crate) fn without_featuring(words: &[String]) -> &[String] {
    let end = words
        .iter()
        .position(|w| matches!(w.as_str(), "feat" | "ft" | "featuring"))
        .unwrap_or(words.len());
    &words[..end]
}

/// "4:05" / "1:02:03" → seconds.
pub(crate) fn parse_clock(s: &str) -> Option<f64> {
    let mut total = 0u64;
    let mut parts = 0;
    for part in s.trim().split(':') {
        if part.is_empty()
            || part.len() > 2 && parts > 0
            || !part.bytes().all(|b| b.is_ascii_digit())
        {
            return None;
        }
        total = total.checked_mul(60)?.checked_add(part.parse().ok()?)?;
        parts += 1;
    }
    (1..=3).contains(&parts).then_some(total as f64)
}

/// "1,234,567 views" → 1234567. "No views" → 0.
pub(crate) fn parse_count(s: &str) -> Option<u64> {
    let digits: String = s.chars().filter(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return s.to_ascii_lowercase().starts_with("no ").then_some(0);
    }
    digits.parse().ok()
}

/// "m:ss" for a positive number of seconds.
pub(crate) fn format_clock(secs: f64) -> String {
    let total = secs.abs().round() as u64;
    format!("{}:{:02}", total / 60, total % 60)
}
