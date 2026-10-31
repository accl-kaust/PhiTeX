//! Names the interpreter looks up often, interned once.

use crate::interp::Names;
use crate::obj::Name;

pub struct N {
    pub notdef: Name,
}

impl N {
    pub fn new(names: &mut Names) -> N {
        N {
            notdef: names.intern(b".notdef"),
        }
    }
}
