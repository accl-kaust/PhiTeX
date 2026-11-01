//! Glue specifications (§150) and glue setting (§109, §646).

use crate::arith::Scaled;

/// An order of infinity (§150): `normal`, `fil`, `fill`, `filll`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Order {
    /// Finite.
    #[default]
    Normal = 0,
    /// First order infinity.
    Fil = 1,
    /// Second order infinity.
    Fill = 2,
    /// Third order infinity.
    Filll = 3,
}

impl Order {
    /// The four orders, finite first.
    pub const ALL: [Order; 4] = [Order::Normal, Order::Fil, Order::Fill, Order::Filll];
}

/// A glue specification (§150).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Glue {
    /// Natural width.
    pub width: Scaled,
    /// Stretchability.
    pub stretch: Scaled,
    /// Order of infinity of the stretch.
    pub stretch_order: Order,
    /// Shrinkability.
    pub shrink: Scaled,
    /// Order of infinity of the shrink.
    pub shrink_order: Order,
}

impl Glue {
    /// `zero_glue` (§162).
    pub const ZERO: Glue = Glue {
        width: 0,
        stretch: 0,
        stretch_order: Order::Normal,
        shrink: 0,
        shrink_order: Order::Normal,
    };

    /// Finite glue `w plus st minus sh`.
    #[must_use]
    pub const fn new(width: Scaled, stretch: Scaled, shrink: Scaled) -> Glue {
        Glue {
            width,
            stretch,
            stretch_order: Order::Normal,
            shrink,
            shrink_order: Order::Normal,
        }
    }

    /// A fixed width, no stretch or shrink.
    #[must_use]
    pub const fn fixed(width: Scaled) -> Glue {
        Glue::new(width, 0, 0)
    }
}

/// Totals of stretch or shrink per order of infinity (§646).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OrderTotals(pub [Scaled; 4]);

impl OrderTotals {
    /// Adds `amount` of order `o`.
    pub fn add(&mut self, o: Order, amount: Scaled) {
        self.0[o as usize] += amount;
    }
    /// The amount of order `o`.
    #[must_use]
    pub fn get(&self, o: Order) -> Scaled {
        self.0[o as usize]
    }
    /// The highest order with a nonzero total (§659, §665).
    #[must_use]
    pub fn highest(&self) -> Order {
        if self.0[3] != 0 {
            Order::Filll
        } else if self.0[2] != 0 {
            Order::Fill
        } else if self.0[1] != 0 {
            Order::Fil
        } else {
            Order::Normal
        }
    }
}

/// How a box's glue is set (§135).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum GlueSign {
    /// Glue keeps its natural size.
    #[default]
    Normal,
    /// Glue of order `glue_order` stretches.
    Stretching,
    /// Glue of order `glue_order` shrinks.
    Shrinking,
}

/// The glue setting of a box: ratio, sign and order (§135). The ratio is a
/// C `double` in web2c's TeX (`texmfmp.h`), so an `f64` here.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GlueSet {
    /// `glue_set`.
    pub ratio: f64,
    /// `glue_sign`.
    pub sign: GlueSign,
    /// `glue_order`.
    pub order: Order,
}
