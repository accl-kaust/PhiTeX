//! A step of the trace: what one traced action of the engine read and
//! wrote. One JSON object per line of the trace file.

use serde::{Deserialize, Serialize};

/// What kind of action a step is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// A primitive command executed (`{\hbox}`).
    Primitive,
    /// A character token typeset or acted on (`{the letter A}`).
    Char,
    /// A font selected (`{select font cmr10}`).
    Font,
    /// A macro (or token-list parameter) expanded (`\foo #1->...`).
    Macro,
    /// A conditional or its `\else`/`\or`/`\fi` (`{\ifnum: (level 1) ...}`).
    If,
    /// A begin-group character.
    GroupBegin,
    /// An end-group character; group restores are its writes.
    GroupEnd,
}

/// Whether a step's following text is consumed in a known extent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Consumes {
    /// The log shows the text consumed (macro arguments, a `\def` body).
    Known,
    /// The step consumes following tokens, but the log doesn't show how
    /// many (`\csname`, `\expandafter`, `\the`...).
    Pending,
}

/// A write of a field.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Write {
    /// The field written (`count:12`, `cs:\foo`, ...).
    pub field: String,
    /// A `\global` assignment (or a `retaining` at a group end).
    pub global: bool,
    /// The stable hash of the value written, as the log prints it.
    pub value: u64,
}

/// A macro call.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MacroCall {
    /// The macro's name (`\foo`), or a token-list parameter's (`\output`).
    pub r#macro: String,
    /// The stable hash of the arguments' text, as the log prints them.
    pub args_text_hash: u64,
    /// How many arguments the call took.
    pub args: u32,
}

/// One step.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Step {
    pub id: u64,
    pub kind: Kind,
    /// The primitive's or macro's name, or the character class.
    pub name: String,
    /// The input line last reported by the log before this step (see the
    /// crate's documentation: the log reports lines at group and
    /// conditional boundaries and in error contexts, not per step).
    pub line: Option<u32>,
    /// The file being read, from the log's parentheses (best effort).
    pub file: Option<String>,
    /// The group level at the step's start.
    pub depth: u32,
    /// Fields read; `"?"` marks reads the log doesn't show.
    pub reads: Vec<String>,
    pub writes: Vec<Write>,
    pub consumes: Option<Consumes>,
    pub macro_call: Option<MacroCall>,
}

/// The first line of a trace file: how it was recorded.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Meta {
    /// The main document's file name, as the log prints it.
    pub main: String,
    pub engine: String,
    pub conservative: bool,
}

/// A line of the trace file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Record {
    Meta { meta: Meta },
    Step(Step),
}
