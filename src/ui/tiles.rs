//! Which of the call's tiles are on screen, and how large, so a camera's
//! pictures are made only for tiles in sight, at their size.

/// The call grid's layout, in logical pixels, as `ParticipantGrid` reports it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct Tiles {
    pub columns: u32,
    pub width: f32,
    pub height: f32,
    pub gap: f32,
    /// The first row's top, and how far the grid is scrolled down.
    pub top: f32,
    pub scrolled: f32,
    /// The grid's height on screen.
    pub shown: f32,
}

impl Tiles {
    /// The size in pixels the tile at `index` shows at, if any of it is in
    /// sight. `index` counts the tiles before it, such as a shared screen.
    pub(super) fn shown_at(&self, index: usize, scale: f32) -> Option<(u32, u32)> {
        if self.columns == 0 || self.width <= 0.0 || self.height <= 0.0 {
            return None;
        }
        let row = (index / self.columns as usize) as f32;
        let top = self.top + row * (self.height + self.gap) - self.scrolled;
        let in_sight = top < self.shown && top + self.height > 0.0;
        let pixels = |side: f32| ((side * scale).round() as u32).max(2);
        in_sight.then(|| (pixels(self.width), pixels(self.height)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_tiles_in_sight_are_shown() {
        let tiles = Tiles {
            columns: 2,
            width: 300.0,
            height: 168.75,
            gap: 12.0,
            top: 24.0,
            scrolled: 0.0,
            shown: 380.0,
        };
        assert_eq!(tiles.shown_at(0, 2.0), Some((600, 338)));
        assert!(tiles.shown_at(3, 1.0).is_some(), "the second row shows");
        assert_eq!(tiles.shown_at(4, 1.0), None, "the third starts below");
        let scrolled = Tiles {
            scrolled: 220.0,
            ..tiles
        };
        assert_eq!(scrolled.shown_at(0, 1.0), None, "scrolled away");
        assert!(scrolled.shown_at(4, 1.0).is_some());
        assert_eq!(Tiles::default().shown_at(0, 1.0), None);
    }
}
