/// A point in logical (non-Retina) screen points, origin at the top-left of the main display.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Point {
    /// Horizontal coordinate.
    pub x: f64,
    /// Vertical coordinate, growing downwards.
    pub y: f64,
}

impl Point {
    /// Creates a point.
    #[must_use]
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    /// Linear interpolation between `self` (t = 0) and `other` (t = 1).
    #[must_use]
    pub fn lerp(self, other: Self, t: f64) -> Self {
        Self::new((other.x - self.x).mul_add(t, self.x), (other.y - self.y).mul_add(t, self.y))
    }
}

/// A width and height in logical points.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Size {
    /// Horizontal extent.
    pub width: f64,
    /// Vertical extent.
    pub height: f64,
}

impl Size {
    /// Creates a size.
    #[must_use]
    pub const fn new(width: f64, height: f64) -> Self {
        Self { width, height }
    }
}

/// An axis-aligned rectangle in logical points.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    /// Top-left corner.
    pub origin: Point,
    /// Extent.
    pub size: Size,
}

impl Rect {
    /// Creates a rectangle from its top-left corner and size.
    #[must_use]
    pub const fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self { origin: Point::new(x, y), size: Size::new(width, height) }
    }

    /// Smallest x coordinate.
    #[must_use]
    pub const fn min_x(&self) -> f64 {
        self.origin.x
    }

    /// Smallest y coordinate.
    #[must_use]
    pub const fn min_y(&self) -> f64 {
        self.origin.y
    }

    /// Largest x coordinate.
    #[must_use]
    pub fn max_x(&self) -> f64 {
        self.origin.x + self.size.width
    }

    /// Largest y coordinate.
    #[must_use]
    pub fn max_y(&self) -> f64 {
        self.origin.y + self.size.height
    }

    /// Center point.
    #[must_use]
    pub fn center(&self) -> Point {
        Point::new(self.size.width.mul_add(0.5, self.origin.x), self.size.height.mul_add(0.5, self.origin.y))
    }

    /// Area; zero for degenerate rectangles.
    #[must_use]
    pub fn area(&self) -> f64 {
        self.size.width.max(0.0) * self.size.height.max(0.0)
    }

    /// True when the rectangle has no area.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.size.width <= 0.0 || self.size.height <= 0.0
    }

    /// True when `point` lies inside the rectangle (inclusive of the top/left edge).
    #[must_use]
    pub fn contains(&self, point: Point) -> bool {
        point.x >= self.min_x() && point.x < self.max_x() && point.y >= self.min_y() && point.y < self.max_y()
    }

    /// The overlapping region of two rectangles, if any.
    #[must_use]
    pub fn intersection(&self, other: &Self) -> Option<Self> {
        let x0 = self.min_x().max(other.min_x());
        let y0 = self.min_y().max(other.min_y());
        let x1 = self.max_x().min(other.max_x());
        let y1 = self.max_y().min(other.max_y());
        (x1 > x0 && y1 > y0).then(|| Self::new(x0, y0, x1 - x0, y1 - y0))
    }

    /// Moves the rectangle by `dx`, `dy`.
    #[must_use]
    pub fn offset(&self, dx: f64, dy: f64) -> Self {
        Self::new(self.origin.x + dx, self.origin.y + dy, self.size.width, self.size.height)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contains_and_center() {
        let r = Rect::new(10.0, 20.0, 100.0, 50.0);
        assert!(r.contains(Point::new(10.0, 20.0)));
        assert!(!r.contains(Point::new(110.0, 20.0)));
        assert_eq!(r.center(), Point::new(60.0, 45.0));
    }

    #[test]
    fn intersection() {
        let a = Rect::new(0.0, 0.0, 10.0, 10.0);
        let b = Rect::new(5.0, 5.0, 10.0, 10.0);
        assert_eq!(a.intersection(&b), Some(Rect::new(5.0, 5.0, 5.0, 5.0)));
        assert_eq!(a.intersection(&Rect::new(20.0, 20.0, 1.0, 1.0)), None);
    }
}
