//! A1: the node store, the environment's representation, and the reducer:
//! `pending(env; input)` → node ; pending(env', input'). Implements `Ctx`.
//!
//! * `EnvRep`: a persistent hash trie (HAMT) of definitions with TeX's levels,
//!   a persistent group chain (each frame holds the old entries its local
//!   definitions shadowed, undone at its end unless the current entry is
//!   global, and its `\aftergroup` tokens) and an independent persistent
//!   list chain (mode and the list-scope keys). `at` names the node that
//!   produced the env (the chain a list's emits are gathered along).
//! * `Graph`: the arena of nodes (each with its predecessor), `reduce_one`
//!   (exactly one node per call), `run`, memo hits.
//! * `Reducing`: the `Ctx` a node is reduced in. Every read of state from
//!   before the node is recorded once with the value seen; reads of what the
//!   node itself made are not. Structure read after the node closed groups or
//!   lists that were open before it is recorded under pseudo keys
//!   (`Key::Other(pk::TAG, …)`) that `env_lookup` / `Graph::holds` answer from
//!   the node's start thunk, so a node's reads always validate it.
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use crate::ctx::Ctx;
use crate::env::{Change, Delta, Env, GroupKind, Mode};
use crate::item::Item;
use crate::prim::{Kind, PrimId};
use crate::term::{Emit, Input, Node, NodeId, Op, Out, Span, Src, TextId, Thunk, TokKind};
use crate::tok::{Names, Tok, Toks};
use crate::val::{Datum, Key, Meaning, Val};

/// No node (a thunk made by `Graph::start` from a fresh env).
pub const NO_NODE: NodeId = NodeId::MAX;
/// `ignore_depth` (tex.web 212).
pub const IGNORE_DEPTH: i32 = -65_536_000;


// One module in three files, owned separately: the environment, the chain, the reducer.
include!("graph/envrep.rs");
include!("graph/chain.rs");
include!("graph/reduce.rs");
