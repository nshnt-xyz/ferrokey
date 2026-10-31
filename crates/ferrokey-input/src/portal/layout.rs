//! Placing absolute positions on the shared screens.
//!
//! Clients send positions as fractions of the shared screen, since they
//! don't know its size. libei wants them in global logical pixels inside one
//! of a device's regions, which can span several monitors; [`Layout`] maps
//! between the two.

/// One screen, in global logical pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Area {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// The screens, laid out as the compositor arranges them.
#[derive(Debug, Clone, Default)]
pub(super) struct Layout {
    areas: Vec<Area>,
}

impl Layout {
    /// Areas with no size are left out.
    pub fn new(areas: impl IntoIterator<Item = Area>) -> Self {
        let areas = areas
            .into_iter()
            .filter(|area| area.width > 0.0 && area.height > 0.0)
            .collect();
        Self { areas }
    }

    /// Maps a fraction of the bounding box of all areas, with `(0, 0)` at
    /// its top left and `(1, 1)` at its bottom right, to global logical
    /// pixels on one of the areas.
    ///
    /// Positions outside `0..=1` are clamped. A position in a gap between
    /// screens of different sizes goes to the nearest point on a screen.
    pub fn locate(&self, x: f32, y: f32) -> Option<(f64, f64)> {
        let first = self.areas.first()?;
        let mut bounds = (
            first.x,
            first.y,
            first.x + first.width,
            first.y + first.height,
        );
        for area in &self.areas[1..] {
            bounds.0 = bounds.0.min(area.x);
            bounds.1 = bounds.1.min(area.y);
            bounds.2 = bounds.2.max(area.x + area.width);
            bounds.3 = bounds.3.max(area.y + area.height);
        }
        let gx = bounds.0 + f64::from(x.clamp(0.0, 1.0)) * (bounds.2 - bounds.0);
        let gy = bounds.1 + f64::from(y.clamp(0.0, 1.0)) * (bounds.3 - bounds.1);

        // Clamping into an area gives the nearest point on it, so the area
        // that needs the least clamping contains the point or is closest.
        self.areas
            .iter()
            .map(|area| {
                // Stay a little inside the far edges, which belong to the
                // next screen over.
                let px = gx.clamp(area.x, area.x + (area.width - 1.0).max(0.0));
                let py = gy.clamp(area.y, area.y + (area.height - 1.0).max(0.0));
                ((px - gx).hypot(py - gy), (px, py))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, point)| point)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(x: f64, y: f64, width: f64, height: f64) -> Area {
        Area {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn maps_onto_one_screen() {
        let layout = Layout::new([area(0.0, 0.0, 1920.0, 1080.0)]);
        assert_eq!(layout.locate(0.5, 0.25), Some((960.0, 270.0)));
        // The far edge and anything past it stays on the screen.
        assert_eq!(layout.locate(1.0, 2.0), Some((1919.0, 1079.0)));
        assert_eq!(layout.locate(-1.0, 0.0), Some((0.0, 0.0)));
    }

    #[test]
    fn maps_across_screens() {
        // A 1080p screen with a taller, offset one to its right, both
        // starting below the global origin.
        let layout = Layout::new([
            area(0.0, 100.0, 1920.0, 1080.0),
            area(1920.0, 0.0, 1000.0, 1500.0),
            area(0.0, 0.0, 0.0, 0.0),
        ]);
        assert_eq!(layout.locate(0.75, 0.5), Some((2190.0, 750.0)));
        // Below the shorter screen goes to its bottom edge.
        assert_eq!(layout.locate(0.125, 1.0), Some((365.0, 1179.0)));
    }

    #[test]
    fn empty_layout_places_nothing() {
        let layout = Layout::new([area(0.0, 0.0, 0.0, 10.0)]);
        assert_eq!(layout.locate(0.5, 0.5), None);
    }
}
