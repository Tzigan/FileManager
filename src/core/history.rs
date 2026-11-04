use std::path::PathBuf;

/// Per-tab back/forward navigation history.
///
/// `back` and `fwd` hold paths to move *to*; the current path is always kept
/// by the caller and pushed back onto the opposite stack when navigating.
#[derive(Debug, Default)]
pub struct History {
    back: Vec<PathBuf>,
    fwd: Vec<PathBuf>,
}

impl History {
    /// Call before leaving `current` for a new, user-chosen path.
    pub fn push(&mut self, current: PathBuf) {
        if self.back.last() != Some(&current) {
            self.back.push(current);
        }
        self.fwd.clear();
    }

    /// Pop the previous location; `current` moves onto the forward stack.
    pub fn back(&mut self, current: PathBuf) -> Option<PathBuf> {
        let prev = self.back.pop()?;
        self.fwd.push(current);
        Some(prev)
    }

    /// Pop the next location; `current` moves onto the back stack.
    pub fn forward(&mut self, current: PathBuf) -> Option<PathBuf> {
        let next = self.fwd.pop()?;
        self.back.push(current);
        Some(next)
    }

    pub fn can_back(&self) -> bool {
        !self.back.is_empty()
    }

    pub fn can_forward(&self) -> bool {
        !self.fwd.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn back_and_forward() {
        let mut h = History::default();
        h.push(p("a")); // a -> b
        h.push(p("b")); // b -> c
        assert!(h.can_back() && !h.can_forward());

        assert_eq!(h.back(p("c")), Some(p("b")));
        assert_eq!(h.back(p("b")), Some(p("a")));
        assert!(!h.can_back() && h.can_forward());

        assert_eq!(h.forward(p("a")), Some(p("b")));
        assert_eq!(h.forward(p("b")), Some(p("c")));
        assert!(!h.can_forward());
    }

    #[test]
    fn new_navigation_clears_forward() {
        let mut h = History::default();
        h.push(p("a")); // a -> b
        h.back(p("b")); // back to a, fwd = [b]
        h.push(p("a")); // a -> x clears fwd
        assert!(!h.can_forward());
        assert_eq!(h.back(p("x")), Some(p("a")));
    }
}
