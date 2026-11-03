//! A rectangle in float pixels, the one shape every layout function returns.

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    pub fn new(x: f32, y: f32, width: f32, height: f32) -> Rect {
        Rect { x, y, width, height }
    }

    /// The rectangle between two opposite corners, in any order.
    pub fn from_corners(first: (f32, f32), second: (f32, f32)) -> Rect {
        let left = first.0.min(second.0);
        let top = first.1.min(second.1);
        Rect::new(left, top, (first.0 - second.0).abs(), (first.1 - second.1).abs())
    }

    pub fn right(&self) -> f32 {
        self.x + self.width
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.height
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }

    pub fn inset(&self, amount: f32) -> Rect {
        Rect::new(self.x + amount, self.y + amount, self.width - amount * 2.0, self.height - amount * 2.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corners_in_any_order_make_the_same_rect() {
        assert_eq!(Rect::from_corners((10.0, 20.0), (4.0, 50.0)), Rect::new(4.0, 20.0, 6.0, 30.0));
    }

    #[test]
    fn contains_is_half_open() {
        let rect = Rect::new(0.0, 0.0, 10.0, 10.0);
        assert!(rect.contains(0.0, 0.0));
        assert!(!rect.contains(10.0, 5.0));
    }
}
