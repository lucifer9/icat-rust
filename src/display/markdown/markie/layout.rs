//! Rectangle geometry shared by diagram layout and collision checks.

/// Represents a rectangular bounding box.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    /// X position of the rect origin
    pub x: f32,
    /// Y position of the rect origin
    pub y: f32,
    /// Width of the rect
    pub w: f32,
    /// Height of the rect
    pub h: f32,
}

impl Rect {
    /// Create a new rect with given position and dimensions.
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    /// Grow the rect by `pad` on every side.
    pub fn expanded(&self, pad: f32) -> Self {
        Self {
            x: self.x - pad,
            y: self.y - pad,
            w: self.w + pad * 2.0,
            h: self.h + pad * 2.0,
        }
    }

    /// Check if this rect overlaps with another.
    pub fn overlaps(&self, other: &Rect) -> bool {
        self.x < other.x + other.w
            && self.x + self.w > other.x
            && self.y < other.y + other.h
            && self.y + self.h > other.y
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rect_overlaps_true() {
        let a = Rect::new(0.0, 0.0, 50.0, 20.0);
        let b = Rect::new(40.0, 10.0, 50.0, 20.0);
        assert!(a.overlaps(&b));
        assert!(b.overlaps(&a));
    }

    #[test]
    fn test_rect_overlaps_false() {
        let a = Rect::new(0.0, 0.0, 50.0, 20.0);
        let b = Rect::new(60.0, 0.0, 50.0, 20.0);
        assert!(!a.overlaps(&b));
        assert!(!b.overlaps(&a));
    }

    #[test]
    fn test_rect_expanded() {
        let rect = Rect::new(10.0, 20.0, 50.0, 30.0);
        let padded = rect.expanded(5.0);
        assert_eq!(padded, Rect::new(5.0, 15.0, 60.0, 40.0));
    }
}
