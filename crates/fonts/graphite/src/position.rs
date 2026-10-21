// Copyright 2010, SIL International, All rights reserved.
// Ported from graphite2 1.3.15 (src/inc/Position.h), LGPL-2.1-or-later.

//! `Position` and `Rect`, in `float` as graphite2 computes them.

use core::ops::{Add, AddAssign, Mul, MulAssign, Sub};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Position {
    pub x: f32,
    pub y: f32,
}

impl Position {
    #[must_use]
    pub const fn new(x: f32, y: f32) -> Position {
        Position { x, y }
    }
}

impl Add for Position {
    type Output = Position;
    fn add(self, a: Position) -> Position {
        Position::new(self.x + a.x, self.y + a.y)
    }
}

impl Sub for Position {
    type Output = Position;
    fn sub(self, a: Position) -> Position {
        Position::new(self.x - a.x, self.y - a.y)
    }
}

impl Mul<f32> for Position {
    type Output = Position;
    fn mul(self, m: f32) -> Position {
        Position::new(self.x * m, self.y * m)
    }
}

impl AddAssign for Position {
    fn add_assign(&mut self, a: Position) {
        self.x += a.x;
        self.y += a.y;
    }
}

impl MulAssign<f32> for Position {
    fn mul_assign(&mut self, m: f32) {
        self.x *= m;
        self.y *= m;
    }
}

/// A box: bottom left and top right. As a `BBox` its fields are
/// (xi, yi, xa, ya); as a `SlantBox`, (si, di, sa, da).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub bl: Position,
    pub tr: Position,
}

impl Rect {
    #[must_use]
    pub const fn new(bl: Position, tr: Position) -> Rect {
        Rect { bl, tr }
    }

    /// `widen`.
    #[must_use]
    pub fn widen(&self, o: &Rect) -> Rect {
        Rect::new(
            Position::new(
                if self.bl.x > o.bl.x { o.bl.x } else { self.bl.x },
                if self.bl.y > o.bl.y { o.bl.y } else { self.bl.y },
            ),
            Position::new(
                if self.tr.x > o.tr.x { self.tr.x } else { o.tr.x },
                if self.tr.y > o.tr.y { self.tr.y } else { o.tr.y },
            ),
        )
    }

    #[must_use]
    pub fn width(&self) -> f32 {
        self.tr.x - self.bl.x
    }

    #[must_use]
    pub fn height(&self) -> f32 {
        self.tr.y - self.bl.y
    }
}

impl Add<Position> for Rect {
    type Output = Rect;
    fn add(self, a: Position) -> Rect {
        Rect::new(
            Position::new(self.bl.x + a.x, self.bl.y + a.y),
            Position::new(self.tr.x + a.x, self.tr.y + a.y),
        )
    }
}

impl Sub<Position> for Rect {
    type Output = Rect;
    fn sub(self, a: Position) -> Rect {
        Rect::new(
            Position::new(self.bl.x - a.x, self.bl.y - a.y),
            Position::new(self.tr.x - a.x, self.tr.y - a.y),
        )
    }
}

impl Mul<f32> for Rect {
    type Output = Rect;
    fn mul(self, m: f32) -> Rect {
        Rect::new(self.bl * m, self.tr * m)
    }
}
