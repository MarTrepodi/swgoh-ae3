//! Multi-selection of table rows: click, shift-click and cmd/ctrl-click, as
//! in a file manager.
//!
//! Rows are identified by a stable id (their index in the Manifest), and
//! ranges follow the order the rows are currently shown in.

use std::collections::BTreeSet;

/// The modifier keys that change what a click does.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ClickModifiers {
    /// Extend from the anchor to the clicked row.
    pub shift: bool,
    /// Cmd on macOS, Ctrl elsewhere: toggle the clicked row (or add the
    /// shift range to the selection instead of replacing it).
    pub command: bool,
}

#[cfg(test)]
impl ClickModifiers {
    pub const NONE: Self = Self {
        shift: false,
        command: false,
    };
    pub const SHIFT: Self = Self {
        shift: true,
        command: false,
    };
    pub const COMMAND: Self = Self {
        shift: false,
        command: true,
    };
}

/// A set of selected row ids plus the anchor that shift-click extends from.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RowSelection {
    selected: BTreeSet<usize>,
    anchor: Option<usize>,
}

impl RowSelection {
    #[must_use]
    pub fn len(&self) -> usize {
        self.selected.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.selected.is_empty()
    }

    #[must_use]
    pub fn contains(&self, id: usize) -> bool {
        self.selected.contains(&id)
    }

    /// The selected id when exactly one row is selected.
    #[must_use]
    pub fn single(&self) -> Option<usize> {
        if self.selected.len() == 1 {
            self.selected.first().copied()
        } else {
            None
        }
    }

    /// Selected ids in ascending order.
    pub fn iter(&self) -> impl Iterator<Item = usize> + '_ {
        self.selected.iter().copied()
    }

    #[cfg(test)]
    #[must_use]
    pub fn anchor(&self) -> Option<usize> {
        self.anchor
    }

    /// Applies a click on row `id`; `visible` is the shown order of rows.
    pub fn click(&mut self, id: usize, visible: &[usize], mods: ClickModifiers) {
        let anchor_pos = self
            .anchor
            .and_then(|a| visible.iter().position(|&v| v == a));
        let clicked_pos = visible.iter().position(|&v| v == id);
        match (mods.shift, anchor_pos, clicked_pos) {
            (true, Some(a), Some(c)) => {
                let (lo, hi) = if a <= c { (a, c) } else { (c, a) };
                if !mods.command {
                    self.selected.clear();
                }
                self.selected.extend(&visible[lo..=hi]);
                // The anchor stays put, so further shift-clicks pivot on it.
            }
            _ if mods.command => {
                if !self.selected.remove(&id) {
                    self.selected.insert(id);
                }
                self.anchor = Some(id);
            }
            _ => {
                self.selected.clear();
                self.selected.insert(id);
                self.anchor = Some(id);
            }
        }
    }

    /// Selects every visible row.
    pub fn select_all(&mut self, visible: &[usize]) {
        self.selected = visible.iter().copied().collect();
        self.anchor = visible.first().copied();
    }

    pub fn clear(&mut self) {
        self.selected.clear();
        self.anchor = None;
    }

    /// Drops selected rows that are no longer shown, such as after the
    /// search or type filter changed. Returns whether anything changed.
    pub fn retain_visible(&mut self, visible: &[usize]) -> bool {
        let shown: BTreeSet<usize> = visible.iter().copied().collect();
        let before = self.selected.len();
        self.selected.retain(|id| shown.contains(id));
        if self.anchor.is_some_and(|a| !shown.contains(&a)) {
            self.anchor = None;
        }
        before != self.selected.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(sel: &RowSelection) -> Vec<usize> {
        sel.iter().collect()
    }

    #[test]
    fn plain_click_selects_only_that_row() {
        let visible = [10, 11, 12, 13];
        let mut sel = RowSelection::default();
        sel.click(11, &visible, ClickModifiers::NONE);
        sel.click(13, &visible, ClickModifiers::NONE);
        assert_eq!(ids(&sel), [13]);
        assert_eq!(sel.single(), Some(13));
        assert_eq!(sel.anchor(), Some(13));
    }

    #[test]
    fn command_click_toggles() {
        let visible = [1, 2, 3];
        let mut sel = RowSelection::default();
        sel.click(1, &visible, ClickModifiers::NONE);
        sel.click(3, &visible, ClickModifiers::COMMAND);
        assert_eq!(ids(&sel), [1, 3]);
        assert_eq!(sel.single(), None);
        sel.click(1, &visible, ClickModifiers::COMMAND);
        assert_eq!(ids(&sel), [3]);
    }

    #[test]
    fn shift_click_selects_a_range_in_visible_order() {
        // Visible order differs from id order (sorted by size, say).
        let visible = [5, 2, 9, 7, 1];
        let mut sel = RowSelection::default();
        sel.click(2, &visible, ClickModifiers::NONE);
        sel.click(7, &visible, ClickModifiers::SHIFT);
        assert_eq!(ids(&sel), [2, 7, 9]);
        // Shift-click again pivots on the same anchor and replaces the range.
        sel.click(5, &visible, ClickModifiers::SHIFT);
        assert_eq!(ids(&sel), [2, 5]);
        assert_eq!(sel.anchor(), Some(2));
    }

    #[test]
    fn shift_command_click_adds_a_range() {
        let visible = [0, 1, 2, 3, 4, 5];
        let mut sel = RowSelection::default();
        sel.click(0, &visible, ClickModifiers::NONE);
        sel.click(4, &visible, ClickModifiers::COMMAND);
        sel.click(
            5,
            &visible,
            ClickModifiers {
                shift: true,
                command: true,
            },
        );
        assert_eq!(ids(&sel), [0, 4, 5]);
    }

    #[test]
    fn shift_click_without_a_visible_anchor_acts_as_a_click() {
        let mut sel = RowSelection::default();
        sel.click(3, &[1, 2, 3], ClickModifiers::SHIFT);
        assert_eq!(ids(&sel), [3]);
        // The anchor is filtered out of view: the next shift-click restarts.
        sel.click(1, &[1, 2], ClickModifiers::SHIFT);
        assert_eq!(ids(&sel), [1]);
    }

    #[test]
    fn select_all_clear_and_retain_visible() {
        let mut sel = RowSelection::default();
        sel.select_all(&[4, 5, 6]);
        assert_eq!(sel.len(), 3);
        assert!(sel.retain_visible(&[5, 6, 7]));
        assert_eq!(ids(&sel), [5, 6]);
        assert_eq!(sel.anchor(), None);
        assert!(!sel.retain_visible(&[5, 6]));
        sel.clear();
        assert!(sel.is_empty());
    }
}
