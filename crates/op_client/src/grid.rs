//! Triangles sorted into the columns of a grid seen from above, so that a
//! lookup or a ray tests a handful of them rather than all: terrain heights
//! (`heights`), water levels (`water`) and inspect mode's picking (`pick`).

/// Cell size, seen from above (m).
pub const CELL_M: f64 = 4.0;

/// Items (triangles) sorted into the columns of a grid of [`CELL_M`] cells
/// seen from above, by their extent in x and z. Each item is listed in
/// every cell its extent touches, in the order the items came.
pub struct ColumnGrid {
    /// x and z of the grid's corner.
    min: [f64; 2],
    cols: usize,
    rows: usize,
    /// The items of cell `c` are `items[starts[c]..starts[c + 1]]`.
    starts: Vec<u32>,
    items: Vec<u32>,
}

/// The cell of a coordinate along one axis of `n` cells from `lo`, clamped
/// to them.
fn cell(v: f64, lo: f64, n: usize) -> usize {
    (((v - lo) / CELL_M).floor().max(0.0) as usize).min(n - 1)
}

impl ColumnGrid {
    /// A grid just large enough for items with these extents, item `i`
    /// spanning `extents[i]` = `[[x0, x1], [z0, z1]]`. Without items it has
    /// no cells.
    pub fn new(extents: &[[[f64; 2]; 2]]) -> ColumnGrid {
        let (mut lo, mut hi) = ([f64::MAX; 2], [f64::MIN; 2]);
        for e in extents {
            for k in 0..2 {
                lo[k] = lo[k].min(e[k][0]);
                hi[k] = hi[k].max(e[k][1]);
            }
        }
        let count = |k: usize| {
            if extents.is_empty() {
                0
            } else {
                (((hi[k] - lo[k]) / CELL_M).ceil() as usize).max(1)
            }
        };
        let (cols, rows) = (count(0), count(1));
        let cells = |e: &[[f64; 2]; 2]| {
            let (c0, c1) = (cell(e[0][0], lo[0], cols), cell(e[0][1], lo[0], cols));
            let (r0, r1) = (cell(e[1][0], lo[1], rows), cell(e[1][1], lo[1], rows));
            (r0..=r1).flat_map(move |r| (c0..=c1).map(move |c| r * cols + c))
        };
        let mut starts = vec![0u32; cols * rows + 1];
        for e in extents {
            for c in cells(e) {
                starts[c + 1] += 1;
            }
        }
        for i in 1..starts.len() {
            starts[i] += starts[i - 1];
        }
        let mut next = starts.clone();
        let mut items = vec![0u32; starts[cols * rows] as usize];
        for (id, e) in extents.iter().enumerate() {
            for c in cells(e) {
                items[next[c] as usize] = id as u32;
                next[c] += 1;
            }
        }
        ColumnGrid {
            min: lo,
            cols,
            rows,
            starts,
            items,
        }
    }

    fn cell_items(&self, i: usize) -> &[u32] {
        &self.items[self.starts[i] as usize..self.starts[i + 1] as usize]
    }

    /// The items listed in the cell over (x, z); `None` outside the grid.
    pub fn at(&self, x: f64, z: f64) -> Option<&[u32]> {
        let c = ((x - self.min[0]) / CELL_M).floor();
        let r = ((z - self.min[1]) / CELL_M).floor();
        let inside = c >= 0.0 && r >= 0.0 && (c as usize) < self.cols && (r as usize) < self.rows;
        inside.then(|| self.cell_items(r as usize * self.cols + c as usize))
    }

    /// Walks the cells a ray passes over from `t0` to `t1`, in order
    /// (Amanatides and Woo, in two dimensions), calling `visit` with each
    /// cell's items and the distance at which the ray leaves the cell,
    /// until `visit` returns true. `o` and `d` are the x and z of the ray's
    /// origin and direction; distances are multiples of its direction. The
    /// walk starts in the cell at `t0`, clamped to the grid.
    pub fn walk(
        &self,
        o: [f64; 2],
        d: [f64; 2],
        t0: f64,
        t1: f64,
        mut visit: impl FnMut(&[u32], f64) -> bool,
    ) {
        if self.cols == 0 || self.rows == 0 {
            return;
        }
        let n = [self.cols, self.rows];
        let mut c = [0, 1].map(|k| cell(o[k] + d[k] * t0, self.min[k], n[k]) as i64);
        let n = n.map(|n| n as i64);
        let (mut step, mut next, mut delta) = ([0i64; 2], [f64::INFINITY; 2], [f64::INFINITY; 2]);
        for k in 0..2 {
            if d[k] != 0.0 {
                step[k] = if d[k] > 0.0 { 1 } else { -1 };
                let edge = self.min[k] + (c[k] + i64::from(d[k] > 0.0)) as f64 * CELL_M;
                next[k] = (edge - o[k]) / d[k];
                delta[k] = CELL_M / d[k].abs();
            }
        }
        loop {
            let exit = next[0].min(next[1]).min(t1);
            if visit(self.cell_items((c[1] * n[0] + c[0]) as usize), exit) || exit >= t1 {
                return;
            }
            let k = usize::from(next[1] <= next[0]);
            c[k] += step[k];
            if c[k] < 0 || c[k] >= n[k] {
                return;
            }
            next[k] += delta[k];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Items are listed in every cell their extent touches; lookups outside
    /// the grid find nothing.
    #[test]
    fn items_are_listed_in_every_cell_they_touch() {
        // A grid from (0, 0) to (12, 8): 3 by 2 cells.
        let grid = ColumnGrid::new(&[
            [[0.0, 1.0], [0.0, 1.0]],
            [[3.0, 9.0], [1.0, 2.0]],
            [[10.0, 12.0], [5.0, 8.0]],
        ]);
        assert_eq!((grid.cols, grid.rows), (3, 2));
        assert_eq!(grid.at(0.5, 0.5), Some(&[0, 1][..]));
        assert_eq!(grid.at(6.0, 1.5), Some(&[1][..]));
        assert_eq!(grid.at(9.0, 1.5), Some(&[1][..]));
        assert_eq!(grid.at(11.0, 7.0), Some(&[2][..]));
        assert_eq!(grid.at(5.0, 6.0), Some(&[][..]));
        for (x, z) in [
            (-0.1, 1.0),
            (1.0, -0.1),
            (12.1, 1.0),
            (1.0, 8.1),
            (f64::NAN, 1.0),
        ] {
            assert_eq!(grid.at(x, z), None, "{x}, {z}");
        }
        let empty = ColumnGrid::new(&[]);
        assert_eq!(empty.at(0.0, 0.0), None);
        let mut visited = 0;
        empty.walk([0.0, 0.0], [1.0, 0.0], 0.0, 10.0, |_, _| {
            visited += 1;
            false
        });
        assert_eq!(visited, 0);
    }

    /// A ray walks the cells it passes over, in order, each with the
    /// distance at which it leaves it.
    #[test]
    fn rays_walk_the_cells_they_pass_over() {
        // 4 by 4 cells from (0, 0), one item per cell, numbered by row.
        let extents: Vec<[[f64; 2]; 2]> = (0..16)
            .map(|i| {
                let (c, r) = (f64::from(i % 4), f64::from(i / 4));
                [[c * 4.0, c * 4.0 + 3.0], [r * 4.0, r * 4.0 + 3.0]]
            })
            .collect();
        let grid = ColumnGrid::new(&extents);
        let walk = |o: [f64; 2], d: [f64; 2], t1: f64| {
            let mut cells = Vec::new();
            grid.walk(o, d, 0.0, t1, |items, exit| {
                cells.push((items.to_vec(), exit));
                false
            });
            cells
        };
        let item = |c: u32, r: u32| vec![r * 4 + c];
        // Along a row, from outside the grid, to its far edge.
        assert_eq!(
            walk([-2.0, 6.0], [1.0, 0.0], 100.0),
            [
                (item(0, 1), 6.0),
                (item(1, 1), 10.0),
                (item(2, 1), 14.0),
                (item(3, 1), 18.0)
            ]
        );
        // Diagonally down a column and across, stopping at t1.
        assert_eq!(
            walk([5.0, 14.0], [0.5, -1.0], 5.0),
            [(item(1, 3), 2.0), (item(1, 2), 5.0)]
        );
        // Straight down: one cell.
        assert_eq!(walk([9.0, 9.0], [0.0, 0.0], 50.0), [(item(2, 2), 50.0)]);
        // `visit` stops the walk.
        let mut seen = 0;
        grid.walk([-2.0, 6.0], [1.0, 0.0], 0.0, 100.0, |_, _| {
            seen += 1;
            seen == 2
        });
        assert_eq!(seen, 2);
    }
}
