#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Direction {
    Left,
    Down,
    Up,
    Right,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Layout {
    rows: usize,
    columns: usize,
    count: usize,
}

impl Layout {
    pub(crate) fn new(width: u16, count: usize) -> Self {
        let requested_columns = if width >= 100 {
            3
        } else if width >= 60 {
            2
        } else {
            1
        };
        let rows = count.div_ceil(requested_columns).max(1);
        let columns = count.div_ceil(rows);
        Self {
            rows,
            columns,
            count,
        }
    }

    pub(crate) fn rows(self) -> usize {
        self.rows
    }

    pub(crate) fn columns(self) -> usize {
        self.columns
    }

    pub(crate) fn index(self, column: usize, row: usize) -> Option<usize> {
        let index = column.checked_mul(self.rows)?.checked_add(row)?;
        (column < self.columns && row < self.rows && index < self.count).then_some(index)
    }

    pub(crate) fn position(self, selected: usize) -> Option<(usize, usize)> {
        if self.count == 0 {
            return None;
        }
        let selected = selected.min(self.count - 1);
        Some((selected / self.rows, selected % self.rows))
    }

    pub(crate) fn move_selection(self, selected: usize, direction: Direction) -> Option<usize> {
        let (column, row) = self.position(selected)?;
        let column_start = column * self.rows;
        let column_len = self.rows.min(self.count - column_start);
        let target = match direction {
            Direction::Up => column_start + (row + column_len - 1) % column_len,
            Direction::Down => column_start + (row + 1) % column_len,
            Direction::Left if column > 0 => {
                let target_column = column - 1;
                let target_start = target_column * self.rows;
                target_start + row.min(self.rows.min(self.count - target_start) - 1)
            }
            Direction::Right if column + 1 < self.columns => {
                let target_column = column + 1;
                let target_start = target_column * self.rows;
                target_start + row.min(self.rows.min(self.count - target_start) - 1)
            }
            Direction::Left | Direction::Right => selected.min(self.count - 1),
        };
        Some(target)
    }
}

#[cfg(test)]
mod tests {
    use super::{Direction, Layout};

    #[test]
    fn four_way_navigation_uses_visual_columns_and_clamps_short_last_column() {
        let layout = Layout::new(120, 8);
        assert_eq!((layout.columns(), layout.rows()), (3, 3));
        for (from, direction, expected) in [
            (0, Direction::Right, 3),
            (3, Direction::Left, 0),
            (0, Direction::Up, 2),
            (2, Direction::Down, 0),
            (2, Direction::Right, 5),
            (5, Direction::Right, 7),
            (7, Direction::Right, 7),
            (7, Direction::Up, 6),
            (6, Direction::Down, 7),
        ] {
            assert_eq!(layout.move_selection(from, direction), Some(expected));
        }
    }

    #[test]
    fn layout_thresholds_and_partial_columns_match_column_major_rendering() {
        assert_eq!(Layout::new(59, 8).columns(), 1);
        assert_eq!(Layout::new(60, 8).columns(), 2);
        assert_eq!(Layout::new(99, 8).columns(), 2);
        assert_eq!(Layout::new(100, 8).columns(), 3);

        let layout = Layout::new(120, 4);
        assert_eq!((layout.columns(), layout.rows()), (2, 2));
        assert_eq!(layout.index(0, 0), Some(0));
        assert_eq!(layout.index(0, 1), Some(1));
        assert_eq!(layout.index(1, 0), Some(2));
        assert_eq!(layout.index(1, 1), Some(3));
        assert_eq!(layout.index(2, 0), None);
    }

    #[test]
    fn empty_single_column_and_out_of_range_selections_are_safe() {
        assert_eq!(
            Layout::new(120, 0).move_selection(0, Direction::Right),
            None
        );
        let single = Layout::new(120, 1);
        assert_eq!(single.move_selection(0, Direction::Left), Some(0));
        assert_eq!(single.move_selection(42, Direction::Down), Some(0));

        let narrow = Layout::new(59, 5);
        assert_eq!(narrow.move_selection(0, Direction::Left), Some(0));
        assert_eq!(narrow.move_selection(4, Direction::Right), Some(4));
        assert_eq!(narrow.move_selection(42, Direction::Up), Some(3));
    }
}
