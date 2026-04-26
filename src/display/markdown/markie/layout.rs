//! Layout types and traits for text bounding box calculations.
//!
//! This module provides:
//! - `GlyphBox`: Represents the actual visual bounds of rendered text
//! - `Rect`: Basic rectangle type for bounds
//! - `TextLayout`: Trait for text layout engines
//! - Collision detection utilities

use crate::display::markdown::markie::TextMeasure;

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

    /// Create a rect with padding applied.
    pub fn with_padding(&self, padding: f32) -> Self {
        Self {
            x: self.x - padding,
            y: self.y - padding,
            w: self.w + padding * 2.0,
            h: self.h + padding * 2.0,
        }
    }

    /// Alias for `with_padding` (matches `RectF::expanded`).
    pub fn expanded(&self, pad: f32) -> Self {
        self.with_padding(pad)
    }

    /// Check if this rect overlaps with another.
    pub fn overlaps(&self, other: &Rect) -> bool {
        self.x < other.x + other.w
            && self.x + self.w > other.x
            && self.y < other.y + other.h
            && self.y + self.h > other.y
    }
}

/// Represents the actual visual bounds of rendered text.
///
/// This struct separates the advance width (how much the cursor moves)
/// from the actual ink bounds (what pixels are drawn), which is essential
/// for proper collision detection and text overlap prevention.
#[derive(Debug, Clone, Copy)]
pub struct GlyphBox {
    /// X position of the glyph origin (baseline start)
    pub x: f32,
    /// Y position of the baseline
    pub y: f32,
    /// Advance width (how much cursor moves after this glyph)
    pub advance_width: f32,
    /// Actual ink bounds (for collision detection)
    pub ink_bounds: Rect,
    /// Font ascent (distance from baseline to top of glyphs)
    pub ascent: f32,
    /// Font descent (distance from baseline to bottom of glyphs)
    pub descent: f32,
}

impl GlyphBox {
    /// Create a new GlyphBox with estimated metrics.
    ///
    /// Note: This uses estimated ascent/descent ratios. For production use,
    /// actual font metrics should be used.
    pub fn new_estimated(x: f32, y: f32, w: f32, font_size: f32) -> Self {
        // Typical font metrics estimates
        let ascent = font_size * 0.8;
        let descent = font_size * 0.2;

        Self {
            x,
            y,
            advance_width: w,
            ink_bounds: Rect::new(x, y - ascent, w, ascent + descent),
            ascent,
            descent,
        }
    }

    /// Get the right edge of the advance width.
    pub fn right(&self) -> f32 {
        self.x + self.advance_width
    }

    /// Get the bottom edge (baseline + descent).
    pub fn bottom(&self) -> f32 {
        self.y + self.descent
    }

    /// Get the top edge (baseline - ascent).
    pub fn top(&self) -> f32 {
        self.y - self.ascent
    }
}

/// Check if two glyph boxes overlap visually (based on ink bounds).
pub fn boxes_overlap(a: &GlyphBox, b: &GlyphBox) -> bool {
    a.ink_bounds.overlaps(&b.ink_bounds)
}

/// Trait for text layout engines.
///
/// This trait defines the interface for measuring and laying out text
/// with proper bounding box information.
pub trait TextLayout {
    /// Measure text and return detailed glyph information.
    ///
    /// Returns a vector of GlyphBoxes, one per logical text unit.
    /// For simple implementations, this may return a single GlyphBox
    /// representing the entire text.
    fn measure_glyphs(
        &mut self,
        text: &str,
        font_size: f32,
        is_code: bool,
        is_bold: bool,
        is_italic: bool,
    ) -> Vec<GlyphBox>;

    /// Layout a line of text tokens.
    ///
    /// Takes a slice of text tokens and lays them out horizontally,
    /// starting at the given position with the given font size.
    fn layout_line(
        &mut self,
        tokens: &[&str],
        max_width: f32,
        start_x: f32,
        baseline_y: f32,
        font_size: f32,
    ) -> Vec<GlyphBox>;
}

/// A text layout engine that uses a TextMeasure implementation.
pub struct TextLayoutEngine<T: TextMeasure> {
    measure: T,
}

impl<T: TextMeasure> TextLayoutEngine<T> {
    /// Create a new layout engine with the given text measure.
    pub fn new(measure: T) -> Self {
        Self { measure }
    }
}

impl<T: TextMeasure> TextLayout for TextLayoutEngine<T> {
    fn measure_glyphs(
        &mut self,
        text: &str,
        font_size: f32,
        is_code: bool,
        is_bold: bool,
        is_italic: bool,
    ) -> Vec<GlyphBox> {
        let (width, _height) = self
            .measure
            .measure_text(text, font_size, is_code, is_bold, is_italic, None);

        vec![GlyphBox::new_estimated(0.0, 0.0, width, font_size)]
    }

    fn layout_line(
        &mut self,
        tokens: &[&str],
        max_width: f32,
        start_x: f32,
        baseline_y: f32,
        font_size: f32,
    ) -> Vec<GlyphBox> {
        let mut boxes = Vec::new();
        let mut current_x = start_x;
        let space_width = font_size * 0.3; // Approximate space width

        for (i, token) in tokens.iter().enumerate() {
            // Add space before token (except first)
            if i > 0 {
                current_x += space_width;
            }

            let glyphs = self.measure_glyphs(token, font_size, false, false, false);
            if let Some(mut g) = glyphs.into_iter().next() {
                // Check if token fits
                if current_x + g.advance_width > max_width {
                    // Token doesn't fit - could implement line wrapping here
                    // For now, we still add it (overflow is handled elsewhere)
                }

                g.x = current_x;
                g.y = baseline_y;
                g.ink_bounds.x = current_x;
                g.ink_bounds.y = baseline_y - g.ascent;

                boxes.push(g);
                current_x += g.advance_width;
            }
        }

        boxes
    }
}

/// Result of finding a label position with scoring information.
#[derive(Debug, Clone, Copy)]
pub struct LabelPosition {
    /// The x coordinate of the label anchor point.
    pub x: f32,
    /// The y coordinate of the label anchor point.
    pub y: f32,
    /// The bounding rect of the placed label.
    pub rect: Rect,
    /// The collision score (lower is better, 0 = no collision).
    pub score: f32,
}

/// Edge label placement with collision avoidance.
///
/// Used by Mermaid diagram rendering to ensure edge labels
/// don't overlap with nodes or other labels.
///
/// Supports both simple offset-based placement and advanced
/// scoring-based placement with candidate search.
pub struct EdgeLabelPlacer {
    /// Regions occupied by nodes (higher collision penalty).
    obstacles: Vec<Rect>,
    /// Regions occupied by other labels (moderate collision penalty).
    labels: Vec<Rect>,
    /// Padding around each region.
    padding: f32,
}

impl EdgeLabelPlacer {
    /// Create a new edge label placer with the given padding between labels.
    pub fn new(padding: f32) -> Self {
        Self {
            obstacles: Vec::new(),
            labels: Vec::new(),
            padding,
        }
    }

    /// Reserve an obstacle region (e.g., a node).
    pub fn reserve_obstacle(&mut self, bbox: Rect) {
        self.obstacles.push(bbox.with_padding(self.padding));
    }

    /// Reserve a label region.
    pub fn reserve_label(&mut self, bbox: Rect) {
        self.labels.push(bbox.with_padding(self.padding));
    }

    /// Calculate collision score for a rect position.
    ///
    /// Returns a score where:
    /// - 0.0 means no collision
    /// - Higher values indicate more/worse collisions
    fn score_rect(&self, r: &Rect) -> f32 {
        let mut score = 0.0;

        // Penalty for going out of bounds (top-left)
        if r.x < 0.0 || r.y < 0.0 {
            score += 1000.0;
        }

        // Penalty for colliding with obstacles (nodes)
        for o in &self.obstacles {
            if r.overlaps(o) {
                score += 140.0;
            }
        }

        // Penalty for colliding with other labels
        for l in &self.labels {
            if r.overlaps(l) {
                score += 220.0;
            }
        }

        score
    }

    /// Find a non-overlapping position for a label using simple offset search.
    ///
    /// Tries the preferred position first, then tries offsetting
    /// vertically and horizontally to find a non-overlapping position.
    pub fn find_position(&self, preferred: (f32, f32), label_size: (f32, f32)) -> (f32, f32) {
        let (x, y) = preferred;
        let (w, h) = label_size;

        // Try the preferred position first
        if !self.collides(x, y, w, h) {
            return (x, y);
        }

        // Try offsetting vertically
        for offset in [10.0, 20.0, 30.0, -10.0, -20.0, -30.0] {
            if !self.collides(x, y + offset, w, h) {
                return (x, y + offset);
            }
        }

        // Try offsetting horizontally
        for offset in [10.0, 20.0, 30.0, -10.0, -20.0, -30.0] {
            if !self.collides(x + offset, y, w, h) {
                return (x + offset, y);
            }
        }

        // Fallback to preferred position
        (x, y)
    }

    /// Find the best position for a label using scoring-based candidate search.
    ///
    /// This is the advanced method used for Mermaid edge labels.
    /// It searches through candidate positions and picks the one with
    /// the lowest collision score, with a penalty for distance from anchor.
    ///
    /// # Arguments
    /// * `anchor` - The preferred anchor point (e.g., edge midpoint)
    /// * `label_size` - The (width, height) of the label
    /// * `candidates` - Iterator of (x, y) candidate positions to try
    /// * `movement_weight` - How much to penalize distance from anchor (default: 2.0)
    ///
    /// # Returns
    /// The best label position found.
    pub fn find_best_position(
        &self,
        anchor: (f32, f32),
        label_size: (f32, f32),
        candidates: impl IntoIterator<Item = (f32, f32)>,
        movement_weight: f32,
    ) -> LabelPosition {
        let (anchor_x, anchor_y) = anchor;
        let (w, h) = label_size;

        let rect_at = |lx: f32, ly: f32| -> Rect { Rect::new(lx - w / 2.0, ly - h + 2.0, w, h) };

        let mut best = LabelPosition {
            x: anchor_x,
            y: anchor_y,
            rect: rect_at(anchor_x, anchor_y),
            score: f32::MAX,
        };

        for (lx, ly) in candidates {
            let r = rect_at(lx, ly);
            let collision_score = self.score_rect(&r);
            let distance = ((lx - anchor_x).powi(2) + (ly - anchor_y).powi(2)).sqrt();
            let total_cost = collision_score + distance * movement_weight;

            // Prefer lower cost, or same cost but closer to anchor
            if total_cost < best.score
                || ((total_cost - best.score).abs() < f32::EPSILON
                    && distance
                        < ((best.x - anchor_x).powi(2) + (best.y - anchor_y).powi(2)).sqrt())
            {
                best = LabelPosition {
                    x: lx,
                    y: ly,
                    rect: r,
                    score: total_cost,
                };
            }
        }

        best
    }

    /// Check if a rectangle at the given position would collide with any occupied region.
    fn collides(&self, x: f32, y: f32, w: f32, h: f32) -> bool {
        let proposed = Rect::new(
            x - self.padding,
            y - self.padding,
            w + self.padding * 2.0,
            h + self.padding * 2.0,
        );
        self.obstacles
            .iter()
            .chain(self.labels.iter())
            .any(|r| proposed.overlaps(r))
    }

    /// Commit a placed label so future searches avoid it.
    pub fn commit_label(&mut self, rect: Rect) {
        self.labels.push(rect.with_padding(self.padding));
    }

    /// Clear all occupied regions.
    pub fn clear(&mut self) {
        self.obstacles.clear();
        self.labels.clear();
    }

    /// Get the number of reserved labels.
    pub fn label_count(&self) -> usize {
        self.labels.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Simple mock TextMeasure for testing
    struct TestMeasure;
    impl TextMeasure for TestMeasure {
        fn measure_text(
            &mut self,
            text: &str,
            font_size: f32,
            _is_code: bool,
            _is_bold: bool,
            _is_italic: bool,
            _max_width: Option<f32>,
        ) -> (f32, f32) {
            (text.len() as f32 * font_size * 0.6, font_size)
        }
    }

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
    fn test_rect_with_padding() {
        let rect = Rect::new(10.0, 20.0, 50.0, 30.0);
        let padded = rect.with_padding(5.0);
        assert_eq!(padded.x, 5.0);
        assert_eq!(padded.y, 15.0);
        assert_eq!(padded.w, 60.0);
        assert_eq!(padded.h, 40.0);
    }

    #[test]
    fn test_glyph_box_new_estimated() {
        let glyph = GlyphBox::new_estimated(0.0, 16.0, 100.0, 16.0);
        assert_eq!(glyph.advance_width, 100.0);
        assert_eq!(glyph.ascent, 16.0 * 0.8);
        assert_eq!(glyph.descent, 16.0 * 0.2);
        assert_eq!(glyph.top(), 16.0 - 16.0 * 0.8);
        assert_eq!(glyph.bottom(), 16.0 + 16.0 * 0.2);
    }

    #[test]
    fn test_boxes_overlap_overlapping() {
        let a = GlyphBox::new_estimated(0.0, 16.0, 50.0, 16.0);
        let b = GlyphBox::new_estimated(40.0, 16.0, 50.0, 16.0);
        // These should overlap since ink bounds extend beyond advance width
        assert!(boxes_overlap(&a, &b) || a.ink_bounds.overlaps(&b.ink_bounds));
    }

    #[test]
    fn test_boxes_overlap_not_overlapping() {
        let a = GlyphBox::new_estimated(0.0, 16.0, 50.0, 16.0);
        let b = GlyphBox::new_estimated(100.0, 16.0, 50.0, 16.0);
        assert!(!boxes_overlap(&a, &b));
    }

    #[test]
    fn test_text_layout_engine_measure_glyphs() {
        let mut engine = TextLayoutEngine::new(TestMeasure);
        let glyphs = engine.measure_glyphs("Hello", 16.0, false, false, false);

        assert_eq!(glyphs.len(), 1);
        assert!(glyphs[0].advance_width > 0.0);
        assert!(glyphs[0].ascent > 0.0);
        assert!(glyphs[0].descent > 0.0);
    }

    #[test]
    fn test_text_layout_engine_layout_line() {
        let mut engine = TextLayoutEngine::new(TestMeasure);
        let boxes = engine.layout_line(&["Hello", "World"], 800.0, 0.0, 16.0, 16.0);

        assert!(boxes.len() >= 2);
        // Second box should start after first with space in between
        assert!(boxes[1].x > boxes[0].right());
    }

    #[test]
    fn test_edge_label_placer_find_position() {
        let mut placer = EdgeLabelPlacer::new(5.0);

        // Reserve a region
        placer.reserve_label(Rect::new(50.0, 50.0, 40.0, 20.0));

        // Preferred position that overlaps should be adjusted
        let (x, y) = placer.find_position((55.0, 55.0), (30.0, 15.0));
        // Should have moved
        assert!(x != 55.0 || y != 55.0);
    }

    #[test]
    fn test_edge_label_placer_non_overlapping() {
        let mut placer = EdgeLabelPlacer::new(5.0);

        // Reserve a region
        placer.reserve_label(Rect::new(50.0, 50.0, 40.0, 20.0));

        // Preferred position far from reserved region should stay
        let (x, y) = placer.find_position((200.0, 200.0), (30.0, 15.0));
        assert_eq!(x, 200.0);
        assert_eq!(y, 200.0);
    }

    #[test]
    fn test_edge_label_placer_clear() {
        let mut placer = EdgeLabelPlacer::new(5.0);

        // Reserve multiple regions
        placer.reserve_label(Rect::new(50.0, 50.0, 40.0, 20.0));
        placer.reserve_label(Rect::new(100.0, 100.0, 40.0, 20.0));

        // Clear all
        placer.clear();

        // Now previously reserved position should be available
        let (x, y) = placer.find_position((55.0, 55.0), (30.0, 15.0));
        assert_eq!(x, 55.0);
        assert_eq!(y, 55.0);
    }

    #[test]
    fn test_edge_label_placer_scoring_prefers_no_collision() {
        let mut placer = EdgeLabelPlacer::new(5.0);

        // Reserve an obstacle
        placer.reserve_obstacle(Rect::new(50.0, 50.0, 40.0, 20.0));

        // Candidates: one collides, one doesn't
        // Using a low movement_weight (0.1) to ensure collision avoidance dominates
        let candidates = [(55.0, 55.0), (150.0, 150.0)];
        let result = placer.find_best_position((100.0, 100.0), (30.0, 15.0), candidates, 0.1);

        // Should pick the non-colliding position
        assert_eq!(result.x, 150.0);
        assert_eq!(result.y, 150.0);
    }

    #[test]
    fn test_edge_label_placer_scoring_with_collision_chooses_best() {
        let mut placer = EdgeLabelPlacer::new(5.0);

        // Reserve obstacles so all candidates collide
        placer.reserve_obstacle(Rect::new(0.0, 0.0, 200.0, 200.0));

        let candidates = [(50.0, 50.0), (100.0, 100.0)];
        let result = placer.find_best_position((75.0, 75.0), (30.0, 15.0), candidates, 2.0);

        // Should pick the one closer to anchor (75, 75) even though both collide
        // Distance from (50, 50) to (75, 75) = ~35
        // Distance from (100, 100) to (75, 75) = ~35
        // Both same distance, so it should pick the first one (50, 50)
        assert_eq!(result.x, 50.0);
        assert_eq!(result.y, 50.0);
    }

    #[test]
    fn test_edge_label_placer_commit_label() {
        let mut placer = EdgeLabelPlacer::new(5.0);

        let candidates = [(55.0, 55.0)];
        let result = placer.find_best_position((55.0, 55.0), (30.0, 15.0), candidates, 2.0);

        // Commit the placed label
        placer.commit_label(result.rect);

        // Now it should be counted
        assert_eq!(placer.label_count(), 1);

        // Future placements should avoid this region
        let candidates2 = [(55.0, 55.0), (200.0, 200.0)];
        let result2 = placer.find_best_position((100.0, 100.0), (30.0, 15.0), candidates2, 2.0);
        assert_eq!(result2.x, 200.0);
    }

    #[test]
    fn test_edge_label_placer_score_rect_no_collision() {
        let placer = EdgeLabelPlacer::new(5.0);
        let r = Rect::new(100.0, 100.0, 30.0, 15.0);

        // Should have zero score when nothing is reserved
        let score = placer.score_rect(&r);
        assert_eq!(score, 0.0);
    }

    #[test]
    fn test_edge_label_placer_score_rect_with_obstacle() {
        let mut placer = EdgeLabelPlacer::new(5.0);
        placer.reserve_obstacle(Rect::new(90.0, 90.0, 50.0, 30.0));

        let r = Rect::new(100.0, 100.0, 30.0, 15.0);
        let score = placer.score_rect(&r);

        // Should have non-zero score due to obstacle collision
        assert!(score > 0.0);
    }

    #[test]
    fn test_edge_label_placer_score_rect_with_label() {
        let mut placer = EdgeLabelPlacer::new(5.0);
        placer.reserve_label(Rect::new(90.0, 90.0, 50.0, 30.0));

        let r = Rect::new(100.0, 100.0, 30.0, 15.0);
        let score = placer.score_rect(&r);

        // Should have non-zero score due to label collision
        assert!(score > 0.0);
    }

    // Property-based tests
    #[test]
    fn test_proptest_rect_overlap_symmetric() {
        use proptest::prelude::*;

        proptest!(|(
            x1 in 0.0f32..200.0, y1 in 0.0f32..200.0, w1 in 10.0f32..100.0, h1 in 10.0f32..100.0,
            x2 in 0.0f32..200.0, y2 in 0.0f32..200.0, w2 in 10.0f32..100.0, h2 in 10.0f32..100.0,
        )| {
            let a = Rect::new(x1, y1, w1, h1);
            let b = Rect::new(x2, y2, w2, h2);

            // Overlap should be symmetric
            prop_assert_eq!(a.overlaps(&b), b.overlaps(&a));
        });
    }

    #[test]
    fn test_proptest_glyph_box_dimensions_positive() {
        use proptest::prelude::*;

        proptest!(|(
            x in 0.0f32..500.0,
            y in 0.0f32..500.0,
            width in 10.0f32..200.0,
            font_size in 8.0f32..48.0,
        )| {
            let glyph = GlyphBox::new_estimated(x, y, width, font_size);

            prop_assert!(glyph.advance_width > 0.0);
            prop_assert!(glyph.ascent > 0.0);
            prop_assert!(glyph.descent > 0.0);
            prop_assert!(glyph.ink_bounds.w > 0.0);
            prop_assert!(glyph.ink_bounds.h > 0.0);
        });
    }

    #[test]
    fn test_proptest_layout_line_increasing_x() {
        use proptest::prelude::*;

        prop_compose! {
            fn arb_word()(s in "[a-zA-Z]{1,10}") -> String {
                s
            }
        }

        proptest!(|(words in prop::collection::vec(arb_word(), 2..5))| {
            let mut engine = TextLayoutEngine::new(TestMeasure);
            let tokens: Vec<&str> = words.iter().map(|s| s.as_str()).collect();
            let boxes = engine.layout_line(&tokens, 800.0, 0.0, 16.0, 16.0);

            // X positions should be strictly increasing
            for i in 1..boxes.len() {
                prop_assert!(
                    boxes[i].x > boxes[i-1].x,
                    "X positions should be increasing: {} should be > {}",
                    boxes[i].x,
                    boxes[i-1].x
                );
            }
        });
    }
}
