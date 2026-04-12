use std::sync::Mutex;

pub struct CalcEngine {
    context: Mutex<fend_core::Context>,
}

struct NoInterrupt;
impl fend_core::Interrupt for NoInterrupt {
    fn should_interrupt(&self) -> bool {
        false
    }
}

impl CalcEngine {
    pub fn new() -> Self {
        let mut ctx = fend_core::Context::new();
        ctx.set_random_u32_fn(|| {
            use std::collections::hash_map::RandomState;
            use std::hash::{BuildHasher, Hasher};
            RandomState::new().build_hasher().finish() as u32
        });
        Self {
            context: Mutex::new(ctx),
        }
    }

    pub fn evaluate(&self, input: &str) -> Option<String> {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return None;
        }
        if !has_calc_signal(trimmed) {
            return None;
        }
        // strip trailing " = <result>" so re-evaluation works on applied lines
        let (expr, applied_result) = split_applied_result(trimmed);
        let mut ctx = self.context.lock().unwrap();
        match fend_core::evaluate_with_interrupt(expr, &mut ctx, &NoInterrupt) {
            Ok(result) => {
                let text = result.get_main_result().to_string();
                if text == expr {
                    return None;
                }
                if applied_result
                    .map(|applied| applied.trim() == text)
                    .unwrap_or(false)
                {
                    return None;
                }
                Some(text)
            }
            Err(_) => None,
        }
    }

    pub fn evaluate_lines(&self, lines: &[String]) -> Vec<Option<String>> {
        lines.iter().map(|line| self.evaluate(line)).collect()
    }
}

fn has_calc_signal(s: &str) -> bool {
    if looks_like_date(s) {
        return false;
    }

    s.bytes().any(|b| {
        matches!(
            b,
            b'+' | b'-' | b'*' | b'/' | b'^' | b'%' | b'(' | b'0'..=b'9'
        )
    }) || s.contains(" to ")
        || s.contains(" in ")
}

fn split3(s: &str, delim: char) -> Option<(&str, &str, &str)> {
    let mut it = s.split(delim);
    let a = it.next()?;
    let b = it.next()?;
    let c = it.next()?;
    if it.next().is_some() {
        return None;
    }
    Some((a, b, c))
}

fn parse_u32_len(part: &str, min_len: usize, max_len: usize) -> Option<u32> {
    if part.len() < min_len || part.len() > max_len || !part.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    part.parse::<u32>().ok()
}

fn looks_like_date_with_delim(s: &str, delim: char) -> bool {
    let Some((a, b, c)) = split3(s, delim) else {
        return false;
    };

    // YYYY-MM-DD
    if parse_u32_len(a, 4, 4).is_some() {
        if let (Some(month), Some(day)) = (parse_u32_len(b, 1, 2), parse_u32_len(c, 1, 2)) {
            if (1..=12).contains(&month) && (1..=31).contains(&day) {
                return true;
            }
        }
    }

    // DD/MM/YYYY or MM/DD/YYYY (accept either to avoid locale coupling).
    if parse_u32_len(c, 4, 4).is_some() {
        if let (Some(first), Some(second)) = (parse_u32_len(a, 1, 2), parse_u32_len(b, 1, 2)) {
            let dmy = (1..=31).contains(&first) && (1..=12).contains(&second);
            let mdy = (1..=12).contains(&first) && (1..=31).contains(&second);
            if dmy || mdy {
                return true;
            }
        }
    }

    false
}

fn looks_like_date(s: &str) -> bool {
    let trimmed = s.trim();
    if trimmed.is_empty() || trimmed.contains(' ') {
        return false;
    }

    looks_like_date_with_delim(trimmed, '-')
        || looks_like_date_with_delim(trimmed, '.')
        || looks_like_date_with_delim(trimmed, '/')
}

fn split_applied_result(s: &str) -> (&str, Option<&str>) {
    // if line ends with " = <something>", evaluate just the left side
    if let Some(idx) = s.rfind(" = ") {
        let left = s[..idx].trim();
        if !left.is_empty() {
            let right = s[idx + 3..].trim();
            if !right.is_empty() {
                return (left, Some(right));
            }
            return (left, None);
        }
    }
    (s, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evaluates_basic_arithmetic() {
        let engine = CalcEngine::new();
        assert_eq!(engine.evaluate("200 * 1.19"), Some("238".to_string()));
    }

    #[test]
    fn ignores_non_expression_lines() {
        let engine = CalcEngine::new();
        assert_eq!(engine.evaluate("todo buy milk"), None);
        assert_eq!(engine.evaluate(""), None);
    }

    #[test]
    fn reevaluates_lines_with_applied_result() {
        let engine = CalcEngine::new();
        assert_eq!(engine.evaluate("2 + 2 = 4"), None);
        assert_eq!(engine.evaluate("2 + 2 = 5"), Some("4".to_string()));
        assert_eq!(split_applied_result("2 + 2 = 4"), ("2 + 2", Some("4")));
    }

    #[test]
    fn evaluates_multiple_lines() {
        let engine = CalcEngine::new();
        let lines = vec![
            "2+2".to_string(),
            "not calc".to_string(),
            "10/2".to_string(),
        ];
        let results = engine.evaluate_lines(&lines);
        assert_eq!(
            results,
            vec![Some("4".to_string()), None, Some("5".to_string())]
        );
    }

    #[test]
    fn ignores_date_like_lines() {
        let engine = CalcEngine::new();
        assert_eq!(engine.evaluate("2026-07-03"), None);
        assert_eq!(engine.evaluate("03.07.2026"), None);
        assert_eq!(engine.evaluate("07/03/2026"), None);
        assert_eq!(engine.evaluate("7/3/2026"), None);
    }

    #[test]
    fn date_detection_does_not_hide_non_date_expressions() {
        assert!(looks_like_date("2026-07-03"));
        assert!(looks_like_date("03.07.2026"));
        assert!(looks_like_date("07/03/2026"));
        assert!(!looks_like_date("2026-07-03 + 2"));
        assert!(!looks_like_date("2 + 2"));
    }
}
