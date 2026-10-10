//! Clip-watch: while it is on, text copied elsewhere is pasted into the
//! open note (`:clip-watch`, `:clip-watch-stop`), as in the terminal app.

#[derive(Debug, Default)]
pub struct ClipWatch {
    /// `Some` while watching; holds the last clipboard text seen.
    last: Option<Option<String>>,
}

impl ClipWatch {
    pub fn enabled(&self) -> bool {
        self.last.is_some()
    }

    /// Start watching; the text on the clipboard now is not pasted.
    /// `false` when already watching.
    pub fn start(&mut self, current: Option<String>) -> bool {
        if self.enabled() {
            return false;
        }
        self.last = Some(current);
        true
    }

    /// `false` when not watching.
    pub fn stop(&mut self) -> bool {
        self.last.take().is_some()
    }

    /// The text to paste for the clipboard's `current` text, if it is new.
    pub fn changed(&mut self, current: Option<String>) -> Option<String> {
        let last = self.last.as_mut()?;
        let text = current.filter(|t| !t.is_empty())?;
        if last.as_deref() == Some(text.as_str()) {
            return None;
        }
        *last = Some(text.clone());
        Some(if text.ends_with('\n') {
            text
        } else {
            format!("{text}\n")
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pastes_new_text_once_each_with_a_newline() {
        let mut w = ClipWatch::default();
        assert_eq!(w.changed(Some("x".into())), None, "off");
        assert!(w.start(Some("old".into())));
        assert!(!w.start(None), "already on");
        assert_eq!(w.changed(Some("old".into())), None, "unchanged");
        assert_eq!(w.changed(None), None);
        assert_eq!(w.changed(Some(String::new())), None);
        assert_eq!(w.changed(Some("new".into())), Some("new\n".into()));
        assert_eq!(w.changed(Some("new".into())), None);
        assert_eq!(w.changed(Some("a\n".into())), Some("a\n".into()));
        assert!(w.stop());
        assert!(!w.stop());
        assert_eq!(w.changed(Some("later".into())), None);
    }
}
