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
        // strip trailing " =" or " = <result>" so re-evaluation works on applied lines
        let expr = strip_applied_result(trimmed);
        let mut ctx = self.context.lock().unwrap();
        match fend_core::evaluate_with_interrupt(expr, &mut ctx, &NoInterrupt) {
            Ok(result) => {
                let text = result.get_main_result().to_string();
                if text == expr {
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
    s.bytes().any(|b| {
        matches!(
            b,
            b'+' | b'-' | b'*' | b'/' | b'^' | b'%' | b'(' | b'0'..=b'9'
        )
    }) || s.contains(" to ")
        || s.contains(" in ")
}

fn strip_applied_result(s: &str) -> &str {
    // if line ends with " = <something>", evaluate just the left side
    if let Some(idx) = s.rfind(" = ") {
        let left = s[..idx].trim();
        if !left.is_empty() {
            return left;
        }
    }
    s
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
        assert_eq!(engine.evaluate("2 + 2 = 4"), Some("4".to_string()));
        assert_eq!(strip_applied_result("2 + 2 = 4"), "2 + 2");
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
}
