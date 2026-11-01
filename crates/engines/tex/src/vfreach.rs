//! The real fonts a virtual font's characters reach. pdfTeX makes a
//! font's object where a character of a real font is first written
//! (`pdf_init_font` at `pdf_set_font`, one object for every font of a
//! TFM name); a virtual font's characters are written through their
//! packets (`do_vf_packet`) in the local fonts they set characters of,
//! and a virtual font is no font of the file: newtx's `ntxsy` and
//! `ntxsy7`, both over `txsys`, make one object.
//!
//! A path names a real font a character reaches from the font it is
//! in: the font's id, then the local font numbers (`fnt_def`'s `k`) of
//! the packets on the way. A sealed line's stand-in lists the paths of
//! its virtual fonts' characters ([`crate::nodes::unit_shape`]), made
//! where the line is; the page's writer resolves them in its own fonts
//! (`crate::pdfout::page_allocs`), as its packets do.

use std::sync::Arc;

use phitex_defuse::{ValueId, Values};
use phitex_typeset::node::Node;

use crate::run::{R, Run};
use crate::val::{int_of, list_of};

/// `vf_max_recursion`: the packets a character goes through at most.
pub(crate) const VF_MAX_RECURSION: usize = 10;

/// The characters packet `p` sets or puts (pdfTeX's `do_vf_packet`),
/// each with the local font number in force: the first of `nums` (the
/// local fonts' numbers in file order) at its start, a font selection
/// naming none of them leaving the one before (`vf_error`); the scan
/// stops at a command `do_vf_packet` does not know.
pub(crate) fn packet_chars(p: &[u8], nums: &[u32]) -> Vec<(u32, u16)> {
    let mut out = Vec::new();
    let mut cur = nums.first().copied();
    let mut i = 0;
    let num = |i: usize, k: usize| -> u32 {
        p.get(i..i + k).map_or(0, |b| {
            b.iter()
                .fold(0u32, |a, &x| a.wrapping_mul(256) | u32::from(x))
        })
    };
    while let Some(&cmd) = p.get(i) {
        i += 1;
        let ch = match cmd {
            0..=127 => Some(u32::from(cmd)),
            128..=131 | 133..=136 => {
                let k = usize::from(if cmd <= 131 { cmd - 127 } else { cmd - 132 });
                i += k;
                Some(num(i - k, k))
            }
            // (set_rule, put_rule)
            132 | 137 => {
                i += 8;
                None
            }
            // (push, pop)
            141 | 142 => None,
            // (right, w, x, down, y, z: their operands; w0, x0, y0, z0
            // none)
            143..=146 => {
                i += usize::from(cmd - 142);
                None
            }
            147..=151 => {
                i += usize::from(cmd - 147);
                None
            }
            152..=156 => {
                i += usize::from(cmd - 152);
                None
            }
            157..=160 => {
                i += usize::from(cmd - 156);
                None
            }
            161..=165 => {
                i += usize::from(cmd - 161);
                None
            }
            166..=170 => {
                i += usize::from(cmd - 166);
                None
            }
            171..=235 => {
                let k = if cmd == 235 {
                    i += 1;
                    num(i - 1, 1)
                } else {
                    u32::from(cmd - 171)
                };
                if nums.contains(&k) {
                    cur = Some(k);
                }
                None
            }
            239..=242 => {
                let n = usize::from(cmd - 238);
                let len = usize::try_from(num(i, n)).unwrap_or(usize::MAX - i - n);
                i += n + len;
                None
            }
            _ => break,
        };
        if let (Some(c), Some(k)) = (ch, cur)
            && let Ok(c) = u16::try_from(c)
        {
            out.push((k, c));
        }
    }
    out
}

/// A virtual font's packets by character and its local fonts by number.
type Virt = (
    Arc<std::collections::BTreeMap<u8, Vec<u8>>>,
    Vec<(u32, VKey)>,
);

/// A font as the reach sees it: the characters it has, and its packets
/// and local fonts where it is virtual.
pub(crate) struct VNode {
    chars: [u64; 4],
    virt: Option<Virt>,
}

impl VNode {
    fn has(&self, c: u16) -> bool {
        usize::from(c) < 256 && (self.chars[usize::from(c) / 64] >> (c % 64)) & 1 == 1
    }
}

/// A font the reach looks up: one of the engine's, or a virtual font's
/// local font by its name (what it is, virtual or real, and its
/// characters are its files', whatever its size).
#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) enum VKey {
    Engine(u32),
    Named(String),
}

/// What a run's reach looked up ([`Run::vf_reach_list`]): the fonts,
/// and each character's paths.
#[derive(Default)]
pub(crate) struct VfCache {
    nodes: crate::FxHashMap<VKey, Arc<VNode>>,
    paths: crate::FxHashMap<(u32, u16), Arc<[Vec<u32>]>>,
}

fn char_bits(l: Option<&crate::fonts::Loaded>) -> [u64; 4] {
    let mut bits = [0u64; 4];
    if let Some(l) = l {
        for c in 0..=255u8 {
            if l.at.char(c).is_some() {
                bits[usize::from(c) / 64] |= 1 << (c % 64);
            }
        }
    }
    bits
}

impl Run<'_, '_> {
    /// Font `k` as the reach sees it, looked up as the page's writer
    /// does (`crate::pdfout::fonts_for`): an engine font's virtual file
    /// by its name, a letter-spaced font's packets over its base, an
    /// auto-expanded font's its base's; a local font's metrics by its
    /// name, then its virtual file.
    fn vnode(&mut self, k: &VKey) -> R<Arc<VNode>> {
        if let Some(n) = self.vf_cache.nodes.get(k) {
            return Ok(n.clone());
        }
        let v = self.v;
        let none = || {
            Arc::new(VNode {
                chars: [0; 4],
                virt: None,
            })
        };
        let n = match k {
            VKey::Engine(f) => {
                let info = crate::fonts::info(v, self.font_get(*f, crate::fields::FontCell::Info)?)
                    .filter(|i| i.file.is_some());
                match info {
                    None => none(),
                    Some(i) => match (i.ex, i.ls) {
                        (Some((b, _, true)), _) => match u32::try_from(b) {
                            Ok(b) => self.vnode(&VKey::Engine(b))?,
                            Err(_) => none(),
                        },
                        (Some(_), _) => Arc::new(VNode {
                            chars: char_bits(crate::fonts::load_info(v, &i).as_deref()),
                            virt: None,
                        }),
                        (None, Some(ls)) => {
                            let l = crate::fonts::load_info(v, &i);
                            match (l, u32::try_from(ls.base)) {
                                (Some(l), Ok(b)) => Arc::new(VNode {
                                    chars: char_bits(Some(&l)),
                                    virt: Some((
                                        Arc::new(ls.packets(&l.tfm)),
                                        vec![(0, VKey::Engine(b))],
                                    )),
                                }),
                                _ => none(),
                            }
                        }
                        (None, None) => {
                            let chars = char_bits(crate::fonts::load_info(v, &i).as_deref());
                            Arc::new(VNode {
                                chars,
                                virt: self.vf_file(&i.name)?,
                            })
                        }
                    },
                }
            }
            VKey::Named(name) => match self.file(&format!("{name}.tfm"))? {
                // (`read_font_info` failing: the null font)
                None => none(),
                Some(file) => {
                    let i = crate::fonts::Info {
                        name: name.clone(),
                        size: 0,
                        dsize: 0,
                        file: Some(file),
                        s: -1000,
                        ls: None,
                        ex: None,
                    };
                    let chars = char_bits(crate::fonts::load_info(v, &i).as_deref());
                    Arc::new(VNode {
                        chars,
                        virt: self.vf_file(name)?,
                    })
                }
            },
        };
        self.vf_cache.nodes.insert(k.clone(), n.clone());
        Ok(n)
    }

    /// Font `name`'s virtual file read, if it has one: its packets and
    /// its local fonts by number.
    fn vf_file(&mut self, name: &str) -> R<Option<Virt>> {
        let Some(b) = crate::pdfout::file_bytes(self, &format!("{name}.vf"))? else {
            return Ok(None);
        };
        let Ok(vf) = phitex_tfm::parse_vf(&b) else {
            return Ok(None);
        };
        let locals = vf
            .fonts
            .iter()
            .map(|lf| {
                (
                    lf.number,
                    VKey::Named(String::from_utf8_lossy(&lf.name).into_owned()),
                )
            })
            .collect();
        let packets = vf
            .chars
            .iter()
            .filter_map(|c| Some((u8::try_from(c.code).ok()?, c.dvi.to_vec())))
            .collect();
        Ok(Some((Arc::new(packets), locals)))
    }

    /// The paths of character `c` of engine font `f` ([`VKey::Engine`]):
    /// none where `f` is real.
    fn char_paths(&mut self, f: u32, c: u16) -> R<Arc<[Vec<u32>]>> {
        if let Some(p) = self.vf_cache.paths.get(&(f, c)) {
            return Ok(p.clone());
        }
        let n = self.vnode(&VKey::Engine(f))?;
        let mut out = std::collections::BTreeSet::new();
        if n.virt.is_some() && n.has(c) {
            self.walk_packet(&n, c, &mut vec![f], 0, &mut out)?;
        }
        let p: Arc<[Vec<u32>]> = out.into_iter().collect();
        self.vf_cache.paths.insert((f, c), p.clone());
        Ok(p)
    }

    /// `do_vf_packet` of character `c` of virtual font `n` at `depth`
    /// (packets gone through before): each real font a character it
    /// sets reaches, by its path (`path` the way to `n`).
    fn walk_packet(
        &mut self,
        n: &VNode,
        c: u16,
        path: &mut Vec<u32>,
        depth: usize,
        out: &mut std::collections::BTreeSet<Vec<u32>>,
    ) -> R<()> {
        if depth >= VF_MAX_RECURSION {
            return Ok(());
        }
        let Some((packets, locals)) = &n.virt else {
            return Ok(());
        };
        let Some(p) = u8::try_from(c).ok().and_then(|c| packets.get(&c)) else {
            return Ok(());
        };
        let nums: Vec<u32> = locals.iter().map(|x| x.0).collect();
        for (k, ch) in packet_chars(p, &nums) {
            let Some(key) = locals.iter().find(|x| x.0 == k).map(|x| x.1.clone()) else {
                continue;
            };
            let g = self.vnode(&key)?;
            // (`do_char`: a character the font does not have is
            // `char_warning`'s, nothing written)
            if !g.has(ch) {
                continue;
            }
            path.push(k);
            if g.virt.is_some() {
                self.walk_packet(&g, ch, path, depth + 1, out)?;
            } else {
                out.insert(path.clone());
            }
            path.pop();
        }
        Ok(())
    }

    /// Which of `ids` are virtual fonts (their characters written
    /// through packets).
    fn virtual_of(&mut self, ids: impl IntoIterator<Item = u32>) -> R<Vec<u32>> {
        let mut out = Vec::new();
        for f in ids {
            if f != 0 && self.vnode(&VKey::Engine(f))?.virt.is_some() {
                out.push(f);
            }
        }
        Ok(out)
    }

    /// The paths of the characters of virtual fonts in `list` (a line
    /// this run made, `ids` its fonts) as written: `UNIT` where it has
    /// none ([`paths_value`]).
    pub(crate) fn vf_reach_nodes(&mut self, list: &[Node], ids: &[u32]) -> R<ValueId> {
        fn walk(l: &[Node], vfs: &[u32], out: &mut std::collections::BTreeSet<(u32, u16)>) {
            for n in l {
                match n {
                    Node::Char { font, ch } if vfs.contains(font) => {
                        out.insert((*font, *ch));
                    }
                    Node::Lig(g) if vfs.contains(&g.font) => {
                        out.insert((g.font, g.ch));
                    }
                    Node::Box(b) => walk(&b.list, vfs, out),
                    Node::Disc(d) => walk(&d.replace, vfs, out),
                    Node::Glue(g) => {
                        if let Some(x) = &g.leader {
                            walk(std::slice::from_ref(&**x), vfs, out);
                        }
                    }
                    _ => {}
                }
            }
        }
        if self.xe {
            return Ok(ValueId::UNIT);
        }
        let vfs = self.virtual_of(ids.iter().copied())?;
        if vfs.is_empty() {
            return Ok(ValueId::UNIT);
        }
        let mut chars = std::collections::BTreeSet::new();
        walk(list, &vfs, &mut chars);
        self.paths_of(chars)
    }

    /// [`Self::vf_reach_nodes`] of a line's list as a value (`l`, its
    /// fonts `fonts`, a list of ids).
    pub(crate) fn vf_reach_list(&mut self, l: ValueId, fonts: ValueId) -> R<ValueId> {
        fn walk(
            v: &Values,
            l: ValueId,
            vfs: &[u32],
            out: &mut std::collections::BTreeSet<(u32, u16)>,
        ) {
            use crate::nodes::{T_BOX, T_CHAR, T_DISC, T_GLUE, T_LIG};
            for n in list_of(v, l) {
                let ys = list_of(v, n);
                let at = |k: usize| ys.get(k).copied().unwrap_or(ValueId::UNIT);
                match ys.first().map(|&t| int_of(v, t)) {
                    Some(T_CHAR | T_LIG) => {
                        if let (Ok(f), Ok(c)) = (
                            u32::try_from(int_of(v, at(1))),
                            u16::try_from(int_of(v, at(2))),
                        ) && vfs.contains(&f)
                        {
                            out.insert((f, c));
                        }
                    }
                    Some(T_BOX) => walk(v, at(9), vfs, out),
                    Some(T_DISC) => walk(v, at(3), vfs, out),
                    Some(T_GLUE) if at(3) != ValueId::UNIT => {
                        walk(v, v.list(vec![at(3)]), vfs, out);
                    }
                    _ => {}
                }
            }
        }
        if self.xe {
            return Ok(ValueId::UNIT);
        }
        let v = self.v;
        let ids: Vec<u32> = list_of(v, fonts)
            .iter()
            .filter_map(|&f| u32::try_from(int_of(v, f)).ok())
            .collect();
        let vfs = self.virtual_of(ids)?;
        if vfs.is_empty() {
            return Ok(ValueId::UNIT);
        }
        let mut chars = std::collections::BTreeSet::new();
        walk(v, l, &vfs, &mut chars);
        self.paths_of(chars)
    }

    fn paths_of(&mut self, chars: std::collections::BTreeSet<(u32, u16)>) -> R<ValueId> {
        let mut all = std::collections::BTreeSet::new();
        for (f, c) in chars {
            all.extend(self.char_paths(f, c)?.iter().cloned());
        }
        Ok(paths_value(self.v, &all))
    }
}

/// Paths as a value: a list of `[font, k...]`, sorted; `UNIT` for none.
pub(crate) fn paths_value(v: &Values, paths: &std::collections::BTreeSet<Vec<u32>>) -> ValueId {
    if paths.is_empty() {
        return ValueId::UNIT;
    }
    v.list(
        paths
            .iter()
            .map(|p| v.list(p.iter().map(|&k| v.int(i64::from(k))).collect()))
            .collect(),
    )
}

/// [`paths_value`] read back.
pub(crate) fn paths_of_value(v: &Values, x: ValueId) -> Vec<Vec<u32>> {
    if x == ValueId::UNIT {
        return Vec::new();
    }
    list_of(v, x)
        .iter()
        .map(|&p| {
            list_of(v, p)
                .iter()
                .filter_map(|&k| u32::try_from(int_of(v, k)).ok())
                .collect()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::packet_chars;

    /// A packet's characters with the local font in force: the first at
    /// the start, a selection, a selection of a font not defined.
    #[test]
    fn packet_chars_follow_font_selections() {
        // set 'A'; right2 0 0; fnt_num_1; set1 200; fnt1 9 (none); put1 66;
        // xxx1 2 "ab"; set 'C'
        let p = [
            65, 144, 0, 0, 172, 128, 200, 235, 9, 133, 66, 239, 2, b'a', b'b', 67,
        ];
        assert_eq!(
            packet_chars(&p, &[0, 1]),
            vec![(0, 65), (1, 200), (1, 66), (1, 67)]
        );
        // (no local font: nothing set)
        assert_eq!(packet_chars(&[65], &[]), Vec::new());
    }
}
