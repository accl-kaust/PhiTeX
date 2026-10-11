//! A1: the node store, the environment's representation, and the reducer:
//! `pending(env; input)` → node ; pending(env', input'). Implements `Ctx`.
use crate::env::{Delta, Env, GroupKind, Mode};
use crate::tok::Toks;
use crate::val::{Key, Val};

/// The environment's representation (A1 decides: e.g. a persistent hash trie
/// for globals plus persistent group and list chains).
#[derive(Debug, Default)]
pub struct EnvRep {}

pub fn env_lookup(_e: &Env, _k: &Key) -> Val { todo!("A1") }
pub fn env_apply(_e: &Env, _d: &Delta) -> Env { todo!("A1") }
pub fn env_group_kind(_e: &Env) -> GroupKind { todo!("A1") }
pub fn env_group_depth(_e: &Env) -> u32 { todo!("A1") }
pub fn env_mode(_e: &Env) -> Mode { todo!("A1") }
pub fn env_list_depth(_e: &Env) -> u32 { todo!("A1") }
pub fn env_after_group(_e: &Env) -> Toks { todo!("A1") }
