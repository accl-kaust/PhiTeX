//! B5: TeX's arithmetic (tex.web 99–108; e-TeX expression rounding).
pub fn half(x: i32) -> i32 { if x & 1 == 1 { (x + 1).div_euclid(2) } else { x / 2 } }
pub fn nx_plus_y(_n: i32, _x: i32, _y: i32) -> Option<i32> { todo!("B5") }
pub fn x_over_n(_x: i32, _n: i32) -> (i32, i32) { todo!("B5") }
pub fn xn_over_d(_x: i32, _n: i32, _d: i32) -> (i32, i32) { todo!("B5") }
pub fn badness(_t: i32, _s: i32) -> i32 { todo!("B5") }
