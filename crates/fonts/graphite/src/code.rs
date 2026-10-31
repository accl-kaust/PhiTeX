// Copyright 2010, SIL International, All rights reserved.
// Ported from graphite2 1.3.15 (src/Code.cpp, src/inc/Code.h,
// src/inc/opcode_table.h), LGPL-2.1-or-later.

//! A rule's constraint or action program, loaded: each byte-code opcode
//! checked and its parameters split off into a data stream, context
//! items' skips counted in instructions and data bytes, and `TEMP_COPY`
//! instructions inserted where an action changes a slot it reads later.

use alloc::vec::Vec;

use crate::segment::attr;

/// The opcodes (`enum opcode`).
#[allow(dead_code)]
pub(crate) mod op {
    pub const NOP: u8 = 0;
    pub const PUSH_BYTE: u8 = 1;
    pub const PUSH_BYTEU: u8 = 2;
    pub const PUSH_SHORT: u8 = 3;
    pub const PUSH_SHORTU: u8 = 4;
    pub const PUSH_LONG: u8 = 5;
    pub const ADD: u8 = 6;
    pub const SUB: u8 = 7;
    pub const MUL: u8 = 8;
    pub const DIV: u8 = 9;
    pub const MIN: u8 = 10;
    pub const MAX: u8 = 11;
    pub const NEG: u8 = 12;
    pub const TRUNC8: u8 = 13;
    pub const TRUNC16: u8 = 14;
    pub const COND: u8 = 15;
    pub const AND: u8 = 16;
    pub const OR: u8 = 17;
    pub const NOT: u8 = 18;
    pub const EQUAL: u8 = 19;
    pub const NOT_EQ: u8 = 20;
    pub const LESS: u8 = 21;
    pub const GTR: u8 = 22;
    pub const LESS_EQ: u8 = 23;
    pub const GTR_EQ: u8 = 24;
    pub const NEXT: u8 = 25;
    pub const NEXT_N: u8 = 26;
    pub const COPY_NEXT: u8 = 27;
    pub const PUT_GLYPH_8BIT_OBS: u8 = 28;
    pub const PUT_SUBS_8BIT_OBS: u8 = 29;
    pub const PUT_COPY: u8 = 30;
    pub const INSERT: u8 = 31;
    pub const DELETE: u8 = 32;
    pub const ASSOC: u8 = 33;
    pub const CNTXT_ITEM: u8 = 34;
    pub const ATTR_SET: u8 = 35;
    pub const ATTR_ADD: u8 = 36;
    pub const ATTR_SUB: u8 = 37;
    pub const ATTR_SET_SLOT: u8 = 38;
    pub const IATTR_SET_SLOT: u8 = 39;
    pub const PUSH_SLOT_ATTR: u8 = 40;
    pub const PUSH_GLYPH_ATTR_OBS: u8 = 41;
    pub const PUSH_GLYPH_METRIC: u8 = 42;
    pub const PUSH_FEAT: u8 = 43;
    pub const PUSH_ATT_TO_GATTR_OBS: u8 = 44;
    pub const PUSH_ATT_TO_GLYPH_METRIC: u8 = 45;
    pub const PUSH_ISLOT_ATTR: u8 = 46;
    pub const PUSH_IGLYPH_ATTR: u8 = 47;
    pub const POP_RET: u8 = 48;
    pub const RET_ZERO: u8 = 49;
    pub const RET_TRUE: u8 = 50;
    pub const IATTR_SET: u8 = 51;
    pub const IATTR_ADD: u8 = 52;
    pub const IATTR_SUB: u8 = 53;
    pub const PUSH_PROC_STATE: u8 = 54;
    pub const PUSH_VERSION: u8 = 55;
    pub const PUT_SUBS: u8 = 56;
    pub const PUT_SUBS2: u8 = 57;
    pub const PUT_SUBS3: u8 = 58;
    pub const PUT_GLYPH: u8 = 59;
    pub const PUSH_GLYPH_ATTR: u8 = 60;
    pub const PUSH_ATT_TO_GLYPH_ATTR: u8 = 61;
    pub const BITOR: u8 = 62;
    pub const BITAND: u8 = 63;
    pub const BITNOT: u8 = 64;
    pub const BITSET: u8 = 65;
    pub const SET_FEAT: u8 = 66;
    pub const MAX_OPCODE: u8 = 67;
    /// (internal, after the opcodes on disk)
    pub const TEMP_COPY: u8 = 67;
}

const VARARGS: u8 = 0xff;

/// `opcode_table`: (parameter bytes, implemented in actions, implemented
/// in constraints).
const TABLE: [(u8, bool, bool); 68] = [
    (0, true, true),        // NOP
    (1, true, true),        // PUSH_BYTE
    (1, true, true),        // PUSH_BYTE_U
    (2, true, true),        // PUSH_SHORT
    (2, true, true),        // PUSH_SHORT_U
    (4, true, true),        // PUSH_LONG
    (0, true, true),        // ADD
    (0, true, true),        // SUB
    (0, true, true),        // MUL
    (0, true, true),        // DIV
    (0, true, true),        // MIN
    (0, true, true),        // MAX
    (0, true, true),        // NEG
    (0, true, true),        // TRUNC8
    (0, true, true),        // TRUNC16
    (0, true, true),        // COND
    (0, true, true),        // AND
    (0, true, true),        // OR
    (0, true, true),        // NOT
    (0, true, true),        // EQUAL
    (0, true, true),        // NOT_EQ
    (0, true, true),        // LESS
    (0, true, true),        // GTR
    (0, true, true),        // LESS_EQ
    (0, true, true),        // GTR_EQ
    (0, true, false),       // NEXT
    (1, false, false),      // NEXT_N
    (0, true, false),       // COPY_NEXT
    (1, true, false),       // PUT_GLYPH_8BIT_OBS
    (3, true, false),       // PUT_SUBS_8BIT_OBS
    (1, true, false),       // PUT_COPY
    (0, true, false),       // INSERT
    (0, true, false),       // DELETE
    (VARARGS, true, false), // ASSOC
    (2, false, true),       // CNTXT_ITEM
    (1, true, false),       // ATTR_SET
    (1, true, false),       // ATTR_ADD
    (1, true, false),       // ATTR_SUB
    (1, true, false),       // ATTR_SET_SLOT
    (2, true, false),       // IATTR_SET_SLOT
    (2, true, true),        // PUSH_SLOT_ATTR
    (2, true, true),        // PUSH_GLYPH_ATTR_OBS
    (3, true, true),        // PUSH_GLYPH_METRIC
    (2, true, true),        // PUSH_FEAT
    (2, true, true),        // PUSH_ATT_TO_GATTR_OBS
    (3, true, true),        // PUSH_ATT_TO_GLYPH_METRIC
    (3, true, true),        // PUSH_ISLOT_ATTR
    (3, false, false),      // PUSH_IGLYPH_ATTR
    (0, true, true),        // POP_RET
    (0, true, true),        // RET_ZERO
    (0, true, true),        // RET_TRUE
    (2, true, false),       // IATTR_SET
    (2, true, false),       // IATTR_ADD
    (2, true, false),       // IATTR_SUB
    (1, true, true),        // PUSH_PROC_STATE
    (0, true, true),        // PUSH_VERSION
    (5, true, false),       // PUT_SUBS
    (0, false, false),      // PUT_SUBS2
    (0, false, false),      // PUT_SUBS3
    (2, true, false),       // PUT_GLYPH
    (3, true, true),        // PUSH_GLYPH_ATTR
    (3, true, true),        // PUSH_ATT_TO_GLYPH_ATTR
    (0, true, true),        // BITOR
    (0, true, true),        // BITAND
    (0, true, true),        // BITNOT
    (4, true, true),        // BITSET
    (2, true, false),       // SET_FEAT
    (0, true, false),       // TEMP_COPY
];

fn implemented(o: u8, constraint: bool) -> bool {
    let t = TABLE[usize::from(o)];
    if constraint { t.2 } else { t.1 }
}

/// `passtype`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(crate) enum PassType {
    Unknown = 0,
    LineBreak,
    Substitute,
    Positioning,
    Justification,
}

/// `Machine::Code::status_t`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[allow(dead_code)]
pub(crate) enum Status {
    Loaded,
    AllocFailed,
    InvalidOpcode,
    UnimplementedOpcodeUsed,
    OutOfRangeData,
    JumpPastEnd,
    ArgumentsExhausted,
    MissingReturn,
    NestedContextItem,
    UnderfullStack,
}

/// A loaded program (`Machine::Code`).
#[derive(Clone, Debug)]
pub(crate) struct Code {
    /// The instructions (opcodes), with the final `RET_ZERO` appended.
    pub instrs: Vec<u8>,
    pub data: Vec<u8>,
    /// Whether there is a program (`_code` non-null).
    pub present: bool,
    pub status: Status,
    pub constraint: bool,
    pub modify: bool,
    pub delete: bool,
    pub max_ref: u8,
}

impl Default for Code {
    fn default() -> Code {
        Code {
            instrs: Vec::new(),
            data: Vec::new(),
            present: false,
            status: Status::Loaded,
            constraint: false,
            modify: false,
            delete: false,
            max_ref: 0,
        }
    }
}

/// The limits a program is checked against (`decoder::limits`).
pub(crate) struct Limits {
    pub pre_context: u8,
    pub rule_length: u16,
    pub classes: u16,
    pub glyf_attrs: u16,
    pub features: u16,
    pub num_user: u8,
}

impl Limits {
    /// `attrid[attr]`: the indices an indexed slot attribute takes.
    fn attrid(&self, a: u8) -> u16 {
        match a {
            15 => 255,
            0..=29 => 1,
            55 => u16::from(self.num_user),
            _ => 0,
        }
    }
}

#[derive(Clone, Copy, Default)]
struct Context {
    changed: bool,
    referenced: bool,
    code_ref: u8,
}

const NUMCONTEXTS: i32 = 256;

struct Decoder<'a> {
    code: &'a mut Code,
    failed: bool,
    bytecode: &'a [u8],
    /// `_max.bytecode`: the end of the bytes being loaded.
    end: usize,
    lim: &'a Limits,
    out_index: i32,
    out_length: u16,
    passtype: PassType,
    stack_depth: i32,
    in_ctxt_item: bool,
    slotref: i16,
    contexts: [Context; NUMCONTEXTS as usize],
    max_ref: u8,
}

impl Decoder<'_> {
    fn failure(&mut self, s: Status) {
        self.code.status = s;
        self.failed = true;
        self.code.present = false;
    }

    fn b(&self, i: usize) -> u8 {
        self.bytecode.get(i).copied().unwrap_or(0)
    }

    /// `valid_upto(limit, x)`.
    fn valid_upto(&mut self, limit: u16, x: u16) -> bool {
        let t = limit != 0 && x < limit;
        if !t {
            self.failure(Status::OutOfRangeData);
        }
        t
    }

    /// `test_ref(index)`.
    fn test_ref(&mut self, index: i8) -> bool {
        let index = i32::from(index);
        if self.code.constraint && !self.in_ctxt_item {
            if index > 0 || -index > i32::from(self.lim.pre_context) {
                self.failure(Status::OutOfRangeData);
                return false;
            }
        } else {
            let r = i32::from(self.slotref) + i32::from(self.lim.pre_context) + index;
            if self.lim.rule_length == 0 || r >= i32::from(self.lim.rule_length) || r < 0 {
                self.failure(Status::OutOfRangeData);
                return false;
            }
        }
        true
    }

    /// `test_context()`.
    fn test_context(&mut self) -> bool {
        if self.out_index >= i32::from(self.out_length)
            || self.out_index < 0
            || i32::from(self.slotref) >= NUMCONTEXTS - 1
        {
            self.failure(Status::OutOfRangeData);
            return false;
        }
        true
    }

    /// `validate_opcode(opc, bc)` (`bc` just past the opcode).
    fn validate_opcode(&mut self, o: u8, bc: usize) -> bool {
        if o >= op::MAX_OPCODE {
            self.failure(Status::InvalidOpcode);
            return false;
        }
        if !implemented(o, self.code.constraint) {
            self.failure(Status::UnimplementedOpcodeUsed);
            return false;
        }
        let psz = TABLE[usize::from(o)].0;
        if psz == VARARGS && bc >= self.end {
            self.failure(Status::ArgumentsExhausted);
            return false;
        }
        let param_sz = if psz == VARARGS {
            usize::from(self.b(bc)) + 1
        } else {
            usize::from(psz)
        };
        if bc - 1 + param_sz >= self.end {
            self.failure(Status::ArgumentsExhausted);
            return false;
        }
        true
    }

    /// `fetch_opcode(bc)`: the opcode at `pos`, or `None` (`MAX_OPCODE`).
    fn fetch_opcode(&mut self, pos: usize) -> Option<u8> {
        let o = self.b(pos);
        let bc = pos + 1;
        if !self.validate_opcode(o, bc) {
            return None;
        }
        let a = |i: usize| self.b(bc + i);
        let (b0, b1, b2, b3, b4) = (a(0), a(1), a(2), a(3), a(4));
        let lim_classes = self.lim.classes;
        match o {
            op::NOP => {}
            op::PUSH_BYTE | op::PUSH_BYTEU | op::PUSH_SHORT | op::PUSH_SHORTU | op::PUSH_LONG => {
                self.stack_depth += 1;
            }
            op::ADD
            | op::SUB
            | op::MUL
            | op::DIV
            | op::MIN
            | op::MAX
            | op::AND
            | op::OR
            | op::EQUAL
            | op::NOT_EQ
            | op::LESS
            | op::GTR
            | op::LESS_EQ
            | op::GTR_EQ
            | op::BITOR
            | op::BITAND => {
                self.stack_depth -= 1;
                if self.stack_depth <= 0 {
                    self.failure(Status::UnderfullStack);
                }
            }
            op::NEG | op::TRUNC8 | op::TRUNC16 | op::NOT | op::BITNOT | op::BITSET => {
                if self.stack_depth <= 0 {
                    self.failure(Status::UnderfullStack);
                }
            }
            op::COND => {
                self.stack_depth -= 2;
                if self.stack_depth <= 0 {
                    self.failure(Status::UnderfullStack);
                }
            }
            op::NEXT_N => {}
            op::NEXT | op::COPY_NEXT => {
                self.out_index += 1;
                if self.out_index < -1
                    || self.out_index > i32::from(self.out_length)
                    || i32::from(self.slotref) > i32::from(self.lim.rule_length)
                {
                    self.failure(Status::OutOfRangeData);
                }
            }
            op::PUT_GLYPH_8BIT_OBS => {
                self.valid_upto(lim_classes, u16::from(b0));
                self.test_context();
            }
            op::PUT_SUBS_8BIT_OBS => {
                self.test_ref(b0 as i8);
                self.valid_upto(lim_classes, u16::from(b1));
                self.valid_upto(lim_classes, u16::from(b2));
                self.test_context();
            }
            op::PUT_COPY => {
                self.test_ref(b0 as i8);
                self.test_context();
            }
            op::INSERT => {
                if self.passtype >= PassType::Positioning {
                    self.failure(Status::InvalidOpcode);
                }
                self.out_length = self.out_length.wrapping_add(1);
                if self.out_index < 0 {
                    self.out_index += 1;
                }
                if self.out_index < -1 || self.out_index >= i32::from(self.out_length) {
                    self.failure(Status::OutOfRangeData);
                }
            }
            op::DELETE => {
                if self.passtype >= PassType::Positioning {
                    self.failure(Status::InvalidOpcode);
                }
                if self.out_index < i32::from(self.lim.pre_context) {
                    self.failure(Status::OutOfRangeData);
                }
                self.out_index -= 1;
                self.out_length = self.out_length.wrapping_sub(1);
                if self.out_index < -1 || self.out_index > i32::from(self.out_length) {
                    self.failure(Status::OutOfRangeData);
                }
            }
            op::ASSOC => {
                if b0 == 0 {
                    self.failure(Status::OutOfRangeData);
                }
                let mut num = b0;
                while num != 0 {
                    let r = self.b(bc + usize::from(num)) as i8;
                    self.test_ref(r);
                    num -= 1;
                }
                self.test_context();
            }
            op::CNTXT_ITEM => {
                self.valid_upto(
                    self.lim.rule_length,
                    (i32::from(self.lim.pre_context) + i32::from(b0 as i8)) as u16,
                );
                if bc + 2 + usize::from(b1) >= self.end {
                    self.failure(Status::JumpPastEnd);
                }
                if self.in_ctxt_item {
                    self.failure(Status::NestedContextItem);
                }
            }
            op::ATTR_SET | op::ATTR_ADD | op::ATTR_SUB | op::ATTR_SET_SLOT => {
                self.stack_depth -= 1;
                if self.stack_depth < 0 {
                    self.failure(Status::UnderfullStack);
                }
                self.valid_upto(u16::from(attr::MAX), u16::from(b0));
                if b0 == attr::USER_DEFN {
                    self.failure(Status::OutOfRangeData);
                }
                self.test_context();
            }
            op::IATTR_SET_SLOT => {
                self.stack_depth -= 1;
                if self.stack_depth < 0 {
                    self.failure(Status::UnderfullStack);
                }
                if self.valid_upto(u16::from(attr::MAX), u16::from(b0)) {
                    let l = self.lim.attrid(b0);
                    self.valid_upto(l, u16::from(b1));
                }
                self.test_context();
            }
            op::PUSH_SLOT_ATTR => {
                self.stack_depth += 1;
                self.valid_upto(u16::from(attr::MAX), u16::from(b0));
                self.test_ref(b1 as i8);
                if b0 == attr::USER_DEFN {
                    self.failure(Status::OutOfRangeData);
                }
            }
            op::PUSH_GLYPH_ATTR_OBS | op::PUSH_ATT_TO_GATTR_OBS => {
                self.stack_depth += 1;
                self.valid_upto(self.lim.glyf_attrs, u16::from(b0));
                self.test_ref(b1 as i8);
            }
            op::PUSH_ATT_TO_GLYPH_METRIC | op::PUSH_GLYPH_METRIC => {
                self.stack_depth += 1;
                // (kgmetDescent)
                self.valid_upto(11, u16::from(b0));
                self.test_ref(b1 as i8);
            }
            op::PUSH_FEAT => {
                self.stack_depth += 1;
                self.valid_upto(self.lim.features, u16::from(b0));
                self.test_ref(b1 as i8);
            }
            op::PUSH_ISLOT_ATTR => {
                self.stack_depth += 1;
                if self.valid_upto(u16::from(attr::MAX), u16::from(b0)) {
                    self.test_ref(b1 as i8);
                    let l = self.lim.attrid(b0);
                    self.valid_upto(l, u16::from(b2));
                }
            }
            op::PUSH_IGLYPH_ATTR => {
                self.stack_depth += 1;
            }
            op::POP_RET => {
                self.stack_depth -= 1;
                if self.stack_depth < 0 {
                    self.failure(Status::UnderfullStack);
                }
            }
            op::RET_ZERO | op::RET_TRUE => {}
            op::IATTR_SET | op::IATTR_ADD | op::IATTR_SUB => {
                self.stack_depth -= 1;
                if self.stack_depth < 0 {
                    self.failure(Status::UnderfullStack);
                }
                if self.valid_upto(u16::from(attr::MAX), u16::from(b0)) {
                    let l = self.lim.attrid(b0);
                    self.valid_upto(l, u16::from(b1));
                }
                self.test_context();
            }
            op::PUSH_PROC_STATE | op::PUSH_VERSION => {
                self.stack_depth += 1;
            }
            op::PUT_SUBS => {
                self.test_ref(b0 as i8);
                self.valid_upto(lim_classes, (u16::from(b1) << 8) | u16::from(b2));
                self.valid_upto(lim_classes, (u16::from(b3) << 8) | u16::from(b4));
                self.test_context();
            }
            op::PUT_SUBS2 | op::PUT_SUBS3 => {}
            op::PUT_GLYPH => {
                self.valid_upto(lim_classes, (u16::from(b0) << 8) | u16::from(b1));
                self.test_context();
            }
            op::PUSH_GLYPH_ATTR | op::PUSH_ATT_TO_GLYPH_ATTR => {
                self.stack_depth += 1;
                self.valid_upto(self.lim.glyf_attrs, (u16::from(b0) << 8) | u16::from(b1));
                self.test_ref(b2 as i8);
            }
            op::SET_FEAT => {
                self.valid_upto(self.lim.features, u16::from(b0));
                self.test_ref(b1 as i8);
            }
            _ => self.failure(Status::InvalidOpcode),
        }
        if self.failed { None } else { Some(o) }
    }

    fn set_ref(&mut self, index: i32) {
        let i = index + i32::from(self.slotref);
        if !(0..NUMCONTEXTS).contains(&i) {
            return;
        }
        self.contexts[i as usize].referenced = true;
        if i > i32::from(self.max_ref) {
            self.max_ref = i as u8;
        }
    }

    fn set_noref(&mut self, index: i32) {
        let i = index + i32::from(self.slotref);
        if !(0..NUMCONTEXTS).contains(&i) {
            return;
        }
        if i > i32::from(self.max_ref) {
            self.max_ref = i as u8;
        }
    }

    fn set_changed(&mut self, index: i32) {
        let i = index + i32::from(self.slotref);
        if !(0..NUMCONTEXTS).contains(&i) {
            return;
        }
        self.contexts[i as usize].changed = true;
        if i > i32::from(self.max_ref) {
            self.max_ref = i as u8;
        }
    }

    /// `analyse_opcode(opc, arg)`.
    fn analyse_opcode(&mut self, o: u8, arg: usize) {
        let a = |d: &Self, i: usize| i32::from(d.b(arg + i) as i8);
        match o {
            op::DELETE => self.code.delete = true,
            op::ASSOC => self.set_changed(0),
            op::PUT_GLYPH_8BIT_OBS | op::PUT_GLYPH => {
                self.code.modify = true;
                self.set_changed(0);
            }
            op::ATTR_SET
            | op::ATTR_ADD
            | op::ATTR_SUB
            | op::ATTR_SET_SLOT
            | op::IATTR_SET_SLOT
            | op::IATTR_SET
            | op::IATTR_ADD
            | op::IATTR_SUB => self.set_noref(0),
            op::NEXT | op::COPY_NEXT => {
                self.slotref += 1;
                let r = self.slotref as usize;
                let cr = (self.code.instrs.len() + 1) as u8;
                if let Some(c) = self.contexts.get_mut(r) {
                    *c = Context {
                        changed: false,
                        referenced: false,
                        code_ref: cr,
                    };
                }
            }
            op::INSERT => {
                if self.slotref >= 0 {
                    self.slotref -= 1;
                }
                self.code.modify = true;
            }
            op::PUT_SUBS_8BIT_OBS | op::PUT_SUBS | op::PUT_COPY => {
                if o != op::PUT_COPY {
                    self.code.modify = true;
                    self.set_changed(0);
                }
                let a0 = a(self, 0);
                if a0 != 0 {
                    self.set_changed(0);
                    self.code.modify = true;
                }
                self.set_ref(a0);
            }
            op::PUSH_GLYPH_ATTR_OBS
            | op::PUSH_SLOT_ATTR
            | op::PUSH_GLYPH_METRIC
            | op::PUSH_ATT_TO_GATTR_OBS
            | op::PUSH_ATT_TO_GLYPH_METRIC
            | op::PUSH_ISLOT_ATTR
            | op::PUSH_FEAT
            | op::SET_FEAT => {
                let a1 = a(self, 1);
                self.set_ref(a1);
            }
            op::PUSH_ATT_TO_GLYPH_ATTR | op::PUSH_GLYPH_ATTR => {
                let a2 = a(self, 2);
                self.set_ref(a2);
            }
            _ => {}
        }
    }

    /// `emit_opcode(opc, bc)`: `bc` past the parameters after.
    fn emit_opcode(&mut self, o: u8, bc: &mut usize) -> bool {
        if !implemented(o, self.code.constraint) {
            self.failure(Status::UnimplementedOpcodeUsed);
            return false;
        }
        let psz = TABLE[usize::from(o)].0;
        let param_sz = if psz == VARARGS {
            usize::from(self.b(*bc)) + 1
        } else {
            usize::from(psz)
        };
        self.code.instrs.push(o);
        if param_sz != 0 {
            for i in 0..param_sz {
                let v = self.b(*bc + i);
                self.code.data.push(v);
            }
            *bc += param_sz;
        }
        if o == op::CNTXT_ITEM {
            self.in_ctxt_item = true;
            let n = self.code.data.len();
            let is_arg = self.code.data[n - 2] as i8;
            self.out_index = i32::from(self.lim.pre_context) + i32::from(is_arg);
            self.slotref = i16::from(is_arg);
            self.out_length = self.lim.rule_length;
            let ctxt_start = self.code.instrs.len();
            let instr_skip_at = n - 1;
            let instr_skip = self.code.data[instr_skip_at];
            let data_skip_at = self.code.data.len();
            self.code.data.push(0);
            let curr_end = self.end;
            if self.load(*bc, *bc + usize::from(instr_skip)) {
                *bc += usize::from(instr_skip);
                let count = (self.code.instrs.len() - ctxt_start) as u8;
                self.code.data[data_skip_at] = instr_skip.wrapping_sub(count);
                self.code.data[instr_skip_at] = count;
                self.end = curr_end;
                self.out_length = 1;
                self.out_index = 0;
                self.slotref = 0;
                self.in_ctxt_item = false;
            } else {
                self.out_index = 0;
                self.slotref = 0;
                return false;
            }
        }
        !self.failed
    }

    /// `load(bc_begin, bc_end)`.
    fn load(&mut self, mut bc: usize, end: usize) -> bool {
        self.end = end;
        while bc < end {
            let Some(o) = self.fetch_opcode(bc) else {
                return false;
            };
            bc += 1;
            self.analyse_opcode(o, bc);
            if !self.emit_opcode(o, &mut bc) {
                return false;
            }
        }
        !self.failed
    }

    /// `apply_analysis`: a `TEMP_COPY` before each instruction that
    /// starts a slot's context where that slot is changed and referenced.
    fn apply_analysis(&mut self) {
        if self.code.constraint {
            return;
        }
        let mut tempcount = 0usize;
        for c in 0..i32::from(self.slotref).max(0) as usize {
            let ctx = self.contexts[c];
            if !ctx.referenced || !ctx.changed {
                continue;
            }
            let tip = usize::from(ctx.code_ref) + tempcount;
            let tip = tip.min(self.code.instrs.len());
            self.code.instrs.insert(tip, op::TEMP_COPY);
            tempcount += 1;
            self.code.delete = true;
        }
    }
}

impl Code {
    /// `Code(is_constraint, bytecode_begin, bytecode_end, pre_context,
    /// rule_length, silf, face, pt)`.
    pub(crate) fn load(constraint: bool, bytecode: &[u8], lim: &Limits, pt: PassType) -> Code {
        let mut code = Code {
            constraint,
            ..Code::default()
        };
        if bytecode.is_empty() {
            return code;
        }
        code.present = true;
        // (an instruction or a datum a byte at most, but for the analysis's
        // copies)
        code.instrs.reserve(bytecode.len());
        code.data.reserve(bytecode.len());
        let (slotref, max_ref, ok) = {
            let mut dec = Decoder {
                code: &mut code,
                failed: false,
                bytecode,
                end: bytecode.len(),
                lim,
                out_index: if constraint {
                    0
                } else {
                    i32::from(lim.pre_context)
                },
                out_length: if constraint { 1 } else { lim.rule_length },
                passtype: pt,
                stack_depth: 0,
                in_ctxt_item: false,
                slotref: 0,
                contexts: [Context::default(); NUMCONTEXTS as usize],
                max_ref: 0,
            };
            let ok = dec.load(0, bytecode.len());
            if ok && !dec.code.instrs.is_empty() {
                let last = *dec.code.instrs.last().expect("instructions");
                if !matches!(last, op::POP_RET | op::RET_ZERO | op::RET_TRUE) {
                    dec.failure(Status::MissingReturn);
                } else {
                    dec.apply_analysis();
                }
            }
            (dec.slotref, dec.max_ref, ok && !dec.failed)
        };
        let _ = slotref;
        if !ok {
            code.present = false;
            code.instrs.clear();
            code.data.clear();
            return code;
        }
        if code.instrs.is_empty() {
            // (an empty program: `Code()`)
            return Code {
                constraint: false,
                ..Code::default()
            };
        }
        code.max_ref = max_ref;
        code.instrs.push(op::RET_ZERO);
        code
    }

    /// `operator bool`.
    pub(crate) fn is_some(&self) -> bool {
        self.present && self.status == Status::Loaded
    }

    /// `immutable()`.
    pub(crate) fn immutable(&self) -> bool {
        !(self.delete || self.modify)
    }
}
