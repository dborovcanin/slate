//! Clock times in calculations.
//!
//! A clock time (`09:10`, `9:10pm`, `9am`) is a point in the day; a range
//! (`09:10-11:45`) is the duration between two of them, wrapping past
//! midnight. Points and durations follow the usual rules: point - point is a
//! duration, point ± duration is a point, and points cannot be added,
//! multiplied or divided.
//!
//! fend has no clock times, so an expression holding one is lowered first:
//! each clock time becomes seconds since midnight and each range its length
//! in seconds, fend computes the result in seconds, and it is written back as
//! a clock time (`11:10`) or a duration (`2h 35min`). Both read back as the
//! same value, so applied results and variables keep working.

const SECONDS_PER_DAY: i64 = 24 * 60 * 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Token {
    /// A clock time, in seconds since midnight.
    Clock(i64),
    /// A range between two clock times, or a duration this module wrote
    /// in parentheses (a variable's value, `(2h 35min)`), in seconds.
    Duration(i64),
    Plus,
    Minus,
    /// `*` or `/`.
    Product,
    Open,
    Close,
    /// Anything else fend reads: numbers, units, names, `%`, `^`, ...
    Text,
}

/// Whether `expr` holds a clock time or a duration in parentheses, so
/// needs [`evaluate`]. Cheap enough for every line: most lines have no `:`
/// or `am`/`pm` after a digit, nor a digit after `(`.
pub(super) fn contains_time(expr: &str) -> bool {
    let bytes = expr.as_bytes();
    let has_marker = bytes.windows(2).any(|pair| {
        (pair[0].is_ascii_digit()
            && (pair[1] == b':' || matches!(pair[1].to_ascii_lowercase(), b'a' | b'p')))
            || (pair[0] == b'(' && (pair[1].is_ascii_digit() || pair[1] == b'-'))
    });
    has_marker
        && tokenize(expr)
            .iter()
            .any(|(token, _)| matches!(token, Token::Clock(_) | Token::Duration(_)))
}

/// Whether `expr` is a single clock time and nothing else. It states a time
/// rather than asking for one, so it gets no result.
pub(super) fn is_bare_clock(expr: &str) -> bool {
    let trimmed = expr.trim();
    lex_clock(trimmed, 0).is_some_and(|(end, _)| end == trimmed.len())
}

/// Whether `text` is a value written by [`evaluate`]: a clock time or a
/// duration. Variables keep such a value whole instead of its first number.
pub(super) fn is_time_value(text: &str) -> bool {
    is_bare_clock(text) || is_duration_value(text)
}

/// Whether `text` is a duration written by [`evaluate`]. A variable holding
/// one is put in parentheses, which marks it as a duration to [`evaluate`].
pub(super) fn is_duration_value(text: &str) -> bool {
    duration_text_seconds(text).is_some()
}

/// Evaluates an expression holding clock times with `eval` (fend). `None`
/// when the expression misuses a clock time, e.g. `09:10 + 11:45`.
pub(super) fn evaluate(expr: &str, mut eval: impl FnMut(&str) -> Option<String>) -> Option<String> {
    let tokens = tokenize(expr);
    let lowered = lower(expr, &tokens);
    // An explicit conversion (`(17:00 - 09:00) to min`) asks fend's format.
    // Only a duration converts: a clock time has no unit to convert to.
    if let Some((at, word)) = conversion_at(expr, &tokens) {
        // The source is everything before the `to`/`in` word, part of the
        // token holding it included; the target is a unit, no time in it.
        let mut source = tokens[..at].to_vec();
        source.push((Token::Text, tokens[at].1.start..word));
        let target_has_time = tokens[at + 1..]
            .iter()
            .any(|(token, _)| matches!(token, Token::Clock(_) | Token::Duration(_)));
        let points = PointCounter::new(expr, &source).count()?;
        return if points == 0 && !target_has_time {
            eval(&lowered)
        } else {
            None
        };
    }
    let points = PointCounter::new(expr, &tokens).count()?;
    let seconds = eval(&format!("({lowered}) to s")).and_then(|text| parse_seconds(&text));
    match (points, seconds) {
        (1, Some(seconds)) => Some(format_clock(seconds)),
        (0, Some(seconds)) => Some(format_duration(seconds)),
        // Durations divided down to a plain number, e.g. `(17:00-09:00) / 4h`.
        (0, None) => eval(&lowered),
        _ => None,
    }
}

fn tokenize(expr: &str) -> Vec<(Token, std::ops::Range<usize>)> {
    let bytes = expr.as_bytes();
    let mut tokens = Vec::new();
    let mut idx = 0;
    let mut text_start: Option<usize> = None;
    while idx < bytes.len() {
        let operator = match bytes[idx] {
            b'+' => Some(Token::Plus),
            b'-' => Some(Token::Minus),
            b'*' | b'/' => Some(Token::Product),
            b'(' => Some(Token::Open),
            b')' => Some(Token::Close),
            _ => None,
        };
        let literal = if operator.is_none() && starts_token(bytes, idx) {
            lex_clock_or_range(expr, idx)
        } else if bytes[idx] == b'(' {
            lex_duration_group(expr, idx)
        } else {
            None
        };
        let operator = if literal.is_some() { None } else { operator };
        if operator.is_none() && literal.is_none() {
            text_start.get_or_insert(idx);
            idx += expr[idx..].chars().next().map_or(1, char::len_utf8);
            continue;
        }
        if let Some(start) = text_start.take() {
            tokens.push((Token::Text, start..idx));
        }
        if let Some((token, end)) = literal {
            tokens.push((token, idx..end));
            idx = end;
        } else if let Some(token) = operator {
            tokens.push((token, idx..idx + 1));
            idx += 1;
        }
    }
    if let Some(start) = text_start {
        tokens.push((Token::Text, start..bytes.len()));
    }
    tokens
}

/// A clock time can start here: not inside a number or a word.
fn starts_token(bytes: &[u8], idx: usize) -> bool {
    bytes[idx].is_ascii_digit()
        && idx
            .checked_sub(1)
            .and_then(|prev| bytes.get(prev))
            .is_none_or(|&prev| {
                !(prev.is_ascii_alphanumeric() || matches!(prev, b'.' | b':' | b'_'))
            })
}

fn lex_clock_or_range(expr: &str, start: usize) -> Option<(Token, usize)> {
    let (end, from) = lex_clock(expr, start)?;
    // `09:10-11:45`, with no spaces around the dash, reads as a range;
    // `11:45 - 09:10` is a subtraction.
    if expr.as_bytes().get(end) == Some(&b'-') {
        if let Some((range_end, to)) = lex_clock(expr, end + 1) {
            // Only a range ending before it starts runs past midnight;
            // `00:00-24:00` is a whole day.
            let length = if to < from {
                to - from + SECONDS_PER_DAY
            } else {
                to - from
            };
            return Some((Token::Duration(length), range_end));
        }
    }
    Some((Token::Clock(from), end))
}

/// Reads a duration this module wrote, in parentheses: `(2h 35min)`.
fn lex_duration_group(expr: &str, start: usize) -> Option<(Token, usize)> {
    let close = start + 1 + expr[start + 1..].find(')')?;
    // `-(2h 35min)` holds its own parentheses.
    let close = if expr[start + 1..].starts_with("-(") {
        close + 1 + expr[close + 1..].find(')')?
    } else {
        close
    };
    let seconds = duration_text_seconds(&expr[start + 1..close])?;
    Some((Token::Duration(seconds), close + 1))
}

/// Reads `H:MM`, `H:MM:SS`, either with `am`/`pm` (attached or after one
/// space), or `Ham`/`Hpm`, returning where it ends and its seconds since
/// midnight. A bare hour needs `am`/`pm` attached, so `5 pm` stays fend's
/// picometres.
fn lex_clock(expr: &str, start: usize) -> Option<(usize, i64)> {
    let bytes = expr.as_bytes();
    let (hour, mut idx) = read_digits(bytes, start, 1, 2)?;
    let mut minute = None;
    let mut second = 0;
    if bytes.get(idx) == Some(&b':') {
        let (value, next) = read_digits(bytes, idx + 1, 2, 2)?;
        minute = Some(value);
        idx = next;
        if bytes.get(idx) == Some(&b':') {
            if let Some((value, next)) = read_digits(bytes, idx + 1, 2, 2) {
                second = value;
                idx = next;
            }
        }
    }

    let suffix_at = |at: usize| -> Option<bool> {
        let suffix = bytes.get(at..at + 2)?;
        let is_pm = match suffix.to_ascii_lowercase().as_slice() {
            b"am" => false,
            b"pm" => true,
            _ => return None,
        };
        ends_token(bytes, at + 2).then_some(is_pm)
    };
    let meridiem = match suffix_at(idx) {
        Some(is_pm) => Some((is_pm, idx + 2)),
        None if minute.is_some() && bytes.get(idx) == Some(&b' ') => {
            suffix_at(idx + 1).map(|is_pm| (is_pm, idx + 3))
        }
        None => None,
    };

    let minute = match (minute, meridiem) {
        (Some(minute), _) => minute,
        (None, Some(_)) => 0,
        (None, None) => return None,
    };
    if minute >= 60 || second >= 60 {
        return None;
    }
    let (hour, end) = match meridiem {
        Some((is_pm, end)) => {
            if !(1..=12).contains(&hour) {
                return None;
            }
            (hour % 12 + if is_pm { 12 } else { 0 }, end)
        }
        None => {
            // `24:00` ends a day, for ranges up to midnight.
            if hour > 24 || (hour == 24 && (minute, second) != (0, 0)) {
                return None;
            }
            (hour, idx)
        }
    };
    if !ends_token(bytes, end) {
        return None;
    }
    Some((end, hour * 3600 + minute * 60 + second))
}

fn ends_token(bytes: &[u8], idx: usize) -> bool {
    bytes
        .get(idx)
        .is_none_or(|&next| !(next.is_ascii_alphanumeric() || matches!(next, b'.' | b':' | b'_')))
}

fn read_digits(bytes: &[u8], start: usize, min: usize, max: usize) -> Option<(i64, usize)> {
    let len = bytes
        .get(start..)?
        .iter()
        .take(max + 1)
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    if !(min..=max).contains(&len) {
        return None;
    }
    let digits = std::str::from_utf8(&bytes[start..start + len]).ok()?;
    Some((digits.parse().ok()?, start + len))
}

/// The expression with each clock time and range replaced by seconds.
fn lower(expr: &str, tokens: &[(Token, std::ops::Range<usize>)]) -> String {
    let mut out = String::with_capacity(expr.len() + 16);
    for (token, span) in tokens {
        match token {
            Token::Clock(seconds) | Token::Duration(seconds) => {
                out.push_str(&format!("({seconds} s)"));
            }
            // `x / 4h` is `(x / 4) h` to fend; `x / (4h)` divides by 4h.
            Token::Text if is_quantity(&expr[span.clone()]) => {
                out.push('(');
                out.push_str(&expr[span.clone()]);
                out.push(')');
            }
            _ => out.push_str(&expr[span.clone()]),
        }
    }
    out
}

/// Numbers each followed by a unit, like `4h`, `2.5 hours` or `2h 30min`.
fn is_quantity(text: &str) -> bool {
    let mut rest = text.trim();
    if rest.is_empty() {
        return false;
    }
    while !rest.is_empty() {
        let number = rest
            .bytes()
            .take_while(|byte| byte.is_ascii_digit() || *byte == b'.')
            .count();
        let after_number = rest[number..].trim_start();
        let unit = after_number
            .bytes()
            .take_while(u8::is_ascii_alphabetic)
            .count();
        if number == 0 || unit == 0 {
            return false;
        }
        rest = after_number[unit..].trim_start();
    }
    true
}

/// A top-level `to` or `in` conversion, outside any parentheses: the index
/// of the token holding the word and the word's byte offset in `expr`.
fn conversion_at(expr: &str, tokens: &[(Token, std::ops::Range<usize>)]) -> Option<(usize, usize)> {
    let mut depth = 0usize;
    tokens
        .iter()
        .enumerate()
        .find_map(|(idx, (token, span))| match token {
            Token::Open => {
                depth += 1;
                None
            }
            Token::Close => {
                depth = depth.saturating_sub(1);
                None
            }
            Token::Text if depth == 0 => {
                let text = &expr[span.clone()];
                text.split_whitespace()
                    .find(|word| word.eq_ignore_ascii_case("to") || word.eq_ignore_ascii_case("in"))
                    .map(|word| {
                        (
                            idx,
                            span.start + (word.as_ptr() as usize - text.as_ptr() as usize),
                        )
                    })
            }
            _ => None,
        })
}

/// Counts the clock times an expression adds up to: 1 means the result is a
/// clock time, 0 a duration or number, `None` a misuse (`09:10 + 11:45`,
/// `09:10 * 2`). Everything that is not a clock time counts as 0, whatever
/// it is; fend checks the units.
struct PointCounter {
    tokens: Vec<Token>,
    next: usize,
}

impl PointCounter {
    fn new(expr: &str, tokens: &[(Token, std::ops::Range<usize>)]) -> Self {
        let tokens = tokens
            .iter()
            .filter(|(token, span)| *token != Token::Text || !expr[span.clone()].trim().is_empty())
            .map(|(token, _)| *token)
            .collect();
        Self { tokens, next: 0 }
    }

    fn count(mut self) -> Option<i64> {
        let points = self.sum()?;
        (self.next == self.tokens.len() && matches!(points, 0 | 1)).then_some(points)
    }

    fn peek(&self) -> Option<Token> {
        self.tokens.get(self.next).copied()
    }

    fn sum(&mut self) -> Option<i64> {
        let mut points = self.product()?;
        while let Some(op @ (Token::Plus | Token::Minus)) = self.peek() {
            self.next += 1;
            let rhs = self.product()?;
            points = if op == Token::Plus {
                points + rhs
            } else {
                points - rhs
            };
        }
        Some(points)
    }

    /// Factors joined by `*`, `/` or by standing next to each other (`2 h`,
    /// `round(...)`): no clock time may take part.
    fn product(&mut self) -> Option<i64> {
        let mut points = self.signed()?;
        loop {
            match self.peek() {
                Some(Token::Product) => self.next += 1,
                Some(Token::Text | Token::Open | Token::Clock(_) | Token::Duration(_)) => {}
                _ => return Some(points),
            }
            let rhs = self.signed()?;
            if points != 0 || rhs != 0 {
                return None;
            }
            points = 0;
        }
    }

    fn signed(&mut self) -> Option<i64> {
        match self.peek()? {
            Token::Plus => {
                self.next += 1;
                self.signed()
            }
            Token::Minus => {
                self.next += 1;
                self.signed().map(|points| -points)
            }
            _ => self.primary(),
        }
    }

    fn primary(&mut self) -> Option<i64> {
        let token = self.peek()?;
        self.next += 1;
        match token {
            Token::Clock(_) => Some(1),
            Token::Duration(_) | Token::Text => Some(0),
            Token::Open => {
                let points = self.sum()?;
                (self.peek() == Some(Token::Close)).then(|| self.next += 1)?;
                Some(points)
            }
            _ => None,
        }
    }
}

/// Seconds from fend's `... to s` output, e.g. `9300 s` or
/// `approx. 4114.2857142857 s`.
fn parse_seconds(text: &str) -> Option<i64> {
    let text = text.strip_prefix("approx. ").unwrap_or(text);
    let (number, unit) = text.split_once(' ')?;
    if unit != "s" {
        return None;
    }
    let value = number.replace(',', "").parse::<f64>().ok()?;
    value.is_finite().then(|| value.round() as i64)
}

fn format_clock(seconds: i64) -> String {
    let seconds = seconds.rem_euclid(SECONDS_PER_DAY);
    let (hour, minute, second) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if second == 0 {
        format!("{hour:02}:{minute:02}")
    } else {
        format!("{hour:02}:{minute:02}:{second:02}")
    }
}

fn format_duration(seconds: i64) -> String {
    let total = seconds.unsigned_abs();
    let (hours, minutes, secs) = (total / 3600, total / 60 % 60, total % 60);
    let mut parts = Vec::with_capacity(3);
    if hours > 0 {
        parts.push(format!("{hours}h"));
    }
    if minutes > 0 {
        parts.push(format!("{minutes}min"));
    }
    if secs > 0 {
        parts.push(format!("{secs}s"));
    }
    if parts.is_empty() {
        return "0min".to_string();
    }
    let text = parts.join(" ");
    match (seconds < 0, parts.len()) {
        (false, _) => text,
        (true, 1) => format!("-{text}"),
        // fend reads `-2h 35min` as -2h + 35min.
        (true, _) => format!("-({text})"),
    }
}

/// The seconds in a duration written like [`format_duration`] writes:
/// `2h 35min`, `45min`, `-(1h 5s)`.
fn duration_text_seconds(text: &str) -> Option<i64> {
    let text = text.trim();
    let (negative, text) = match text
        .strip_prefix("-(")
        .and_then(|inner| inner.strip_suffix(')'))
        .or_else(|| text.strip_prefix('-'))
    {
        Some(inner) => (true, inner),
        None => (false, text),
    };
    let mut units = [("h", 3600), ("min", 60), ("s", 1)].iter();
    let mut seconds = 0i64;
    for part in text.split(' ') {
        let digits = part.bytes().take_while(u8::is_ascii_digit).count();
        let (_, scale) = units.find(|(unit, _)| &part[digits..] == *unit)?;
        let value: i64 = part[..digits].parse().ok()?;
        seconds = seconds.checked_add(value.checked_mul(*scale)?)?;
    }
    Some(if negative { -seconds } else { seconds })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clocks(expr: &str) -> Vec<Token> {
        tokenize(expr)
            .into_iter()
            .map(|(token, _)| token)
            .filter(|token| matches!(token, Token::Clock(_) | Token::Duration(_)))
            .collect()
    }

    #[test]
    fn reads_clock_times() {
        assert_eq!(clocks("09:10"), vec![Token::Clock(33_000)]);
        assert_eq!(clocks("9:10:30"), vec![Token::Clock(33_030)]);
        assert_eq!(clocks("9am"), vec![Token::Clock(32_400)]);
        assert_eq!(
            clocks("12am + 12pm"),
            vec![Token::Clock(0), Token::Clock(43_200)]
        );
        assert_eq!(clocks("9:10 PM"), vec![Token::Clock(76_200)]);
        assert_eq!(clocks("24:00"), vec![Token::Clock(86_400)]);
    }

    #[test]
    fn leaves_other_colons_and_units_alone() {
        for text in [
            "24:30",
            "9:60",
            "13pm",
            "5 pm",
            "9:5",
            "ratio 3:2",
            "a1:30",
            "1.5:30",
            "10:30x",
        ] {
            assert_eq!(clocks(text), vec![], "{text}");
        }
    }

    #[test]
    fn reads_ranges_without_spaces_around_the_dash() {
        assert_eq!(clocks("09:10-11:45"), vec![Token::Duration(9_300)]);
        assert_eq!(clocks("9am-5pm"), vec![Token::Duration(28_800)]);
        assert_eq!(clocks("22:00-02:00"), vec![Token::Duration(14_400)]);
        assert_eq!(clocks("00:00-24:00"), vec![Token::Duration(86_400)]);
        assert_eq!(clocks("09:00-09:00"), vec![Token::Duration(0)]);
        assert_eq!(
            clocks("11:45 - 09:10"),
            vec![Token::Clock(42_300), Token::Clock(33_000)]
        );
    }

    fn points(expr: &str) -> Option<i64> {
        PointCounter::new(expr, &tokenize(expr)).count()
    }

    #[test]
    fn counts_points_to_tell_clock_times_from_durations() {
        assert_eq!(points("09:10 + 2h"), Some(1));
        assert_eq!(points("2h + 09:10"), Some(1));
        assert_eq!(points("11:45 - 09:10"), Some(0));
        assert_eq!(points("09:10-11:45 + 13:00-17:30"), Some(0));
        assert_eq!(points("(17:00 - 09:00) / 8"), Some(0));
        assert_eq!(points("round(17:00 - 09:00)"), Some(0));
        assert_eq!(points("09:10 + 11:45"), None);
        assert_eq!(points("09:10 * 2"), None);
        assert_eq!(points("-09:10"), None);
        assert_eq!(points("(09:10"), None);
        assert_eq!(points("09:10 meeting"), None);
    }

    #[test]
    fn formats_clock_times_and_durations() {
        assert_eq!(format_clock(40_200), "11:10");
        assert_eq!(format_clock(86_400 + 3_600), "01:00");
        assert_eq!(format_clock(-3_600), "23:00");
        assert_eq!(format_clock(33_030), "09:10:30");
        assert_eq!(format_duration(9_300), "2h 35min");
        assert_eq!(format_duration(2_700), "45min");
        assert_eq!(format_duration(7_200), "2h");
        assert_eq!(format_duration(4_114), "1h 8min 34s");
        assert_eq!(format_duration(0), "0min");
        assert_eq!(format_duration(-2_700), "-45min");
        assert_eq!(format_duration(-9_300), "-(2h 35min)");
    }

    #[test]
    fn recognizes_its_own_values() {
        for text in [
            "11:10",
            "09:10:30",
            "2h 35min",
            "45min",
            "1h 8min 34s",
            "-45min",
            "-(2h 35min)",
        ] {
            assert!(is_time_value(text), "{text}");
        }
        for text in [
            "2 h", "35 min", "2.5h", "45", "min", "2h35min", "35min 2h", "5km",
        ] {
            assert!(!is_time_value(text), "{text}");
        }
    }

    #[test]
    fn reads_durations_it_wrote_in_parentheses() {
        assert_eq!(clocks("(2h 35min) * 2"), vec![Token::Duration(9_300)]);
        assert_eq!(
            clocks("09:10 + (-(1h 5s))"),
            vec![Token::Clock(33_000), Token::Duration(-3_605)]
        );
        assert_eq!(clocks("(2 + 3) * 4"), vec![]);
        assert!(contains_time("(45min) * 2"));
        assert!(!contains_time("(2 + 3) * 4h"));
    }

    #[test]
    fn wraps_quantities_for_fend() {
        let lowered = |expr: &str| lower(expr, &tokenize(expr));
        assert_eq!(
            lowered("(17:00 - 09:00) / 4h"),
            "((61200 s) - (32400 s)) /( 4h)"
        );
        assert!(is_quantity(" 2.5 hours "));
        assert!(is_quantity("2h 30min"));
        assert!(!is_quantity(" round"));
        assert!(!is_quantity(" 2 "));
        assert!(!is_quantity(" 2 to min"));
    }

    #[test]
    fn converts_only_durations() {
        let fend = |expr: &str| Some(format!("<{expr}>"));
        assert_eq!(
            evaluate("(17:00 - 09:00) to min", fend).as_deref(),
            Some("<((61200 s) - (32400 s)) to min>")
        );
        assert_eq!(
            evaluate("11:45 - 09:10 to min", fend).as_deref(),
            Some("<(42300 s) - (33000 s) to min>")
        );
        for expr in [
            "09:10 + 2h to min",
            "09:10 - 2h in h",
            "09:10 * 2 to min",
            "09:10 + 11:45 in h",
            "09:10 to min",
            "09:00-10:00 to s + (09:00 * 2)",
            "09:00-10:00 to s + 10:00-11:00",
        ] {
            assert_eq!(evaluate(expr, fend), None, "{expr}");
        }
    }

    #[test]
    fn parses_fend_seconds() {
        assert_eq!(parse_seconds("9300 s"), Some(9_300));
        assert_eq!(parse_seconds("approx. 4114.2857142857 s"), Some(4_114));
        assert_eq!(parse_seconds("-2700 s"), Some(-2_700));
        assert_eq!(parse_seconds("2.5 h"), None);
    }
}
