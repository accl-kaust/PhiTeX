//! D2: `\openin` `\read` `\readline` `\closein` `\ifeof` data, `\openout`
//! `\write` `\closeout` `\immediate`, `\message` `\errmessage`, `\special`.
use std::sync::Arc;
use crate::prim::Registry;
pub fn register(_r: &mut Registry) {}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stream { pub name: Arc<str>, pub data: Arc<[u8]>, pub next: usize }
