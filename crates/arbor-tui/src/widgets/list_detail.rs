pub struct ListDetailState {
    pub selected: usize,
    pub count: usize,
}

impl ListDetailState {
    pub fn new() -> Self {
        Self {
            selected: 0,
            count: 0,
        }
    }

    pub fn select_next(&mut self) {
        if self.count > 0 {
            self.selected = (self.selected + 1).min(self.count - 1);
        }
    }

    pub fn select_prev(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    pub fn set_count(&mut self, count: usize) {
        if count == 0 {
            self.selected = 0;
        } else if self.selected >= count {
            self.selected = count - 1;
        }
        self.count = count;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_state(selected: usize, count: usize) -> ListDetailState {
        let mut state = ListDetailState::new();
        state.count = count;
        state.selected = selected;
        state
    }

    #[test]
    fn select_next_advances_and_clamps() {
        let mut state = make_state(0, 3);
        state.select_next();
        assert_eq!(state.selected, 1);
        state.select_next();
        assert_eq!(state.selected, 2);
        state.select_next();
        assert_eq!(state.selected, 2);
    }

    #[test]
    fn select_prev_decrements_and_stops_at_zero() {
        let mut state = make_state(1, 3);
        state.select_prev();
        assert_eq!(state.selected, 0);
        state.select_prev();
        assert_eq!(state.selected, 0);
    }

    #[test]
    fn set_count_clamps_selected() {
        let mut state = make_state(5, 10);
        state.set_count(3);
        assert_eq!(state.selected, 2);
    }

    #[test]
    fn set_count_zero_resets_selected() {
        let mut state = make_state(5, 10);
        state.set_count(0);
        assert_eq!(state.selected, 0);
    }

    #[test]
    fn select_next_noop_when_empty() {
        let mut state = make_state(0, 0);
        state.select_next();
        assert_eq!(state.selected, 0);
    }
}
