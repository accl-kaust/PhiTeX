//! `SyncTeX` (DESIGN 4.5): pdfTeX's `synctex.c` and its change files
//! (`synctex-mem.ch0`, `synctex-rec.ch0`, `synctex-pdf-rec.ch2`), ported.
//!
//! The engine records *events* where pdfTeX calls the controller: a file
//! opened (`synctexstartinput`), a sheet or form begun and ended, and the
//! ship-out walk's boxes, kerns, glue, rules, math nodes, form references
//! and the ends of runs of characters. A [`Machine`], `synctex.c`'s
//! `synctex_ctxt` with its record functions, turns the events into the
//! `.synctex` text, byte for byte pdfTeX's, gzipped at the end as zlib's
//! `gzopen(name, "wb")` writes it (the host's deflate at level 6 between
//! zlib's gzip header and trailer).
//!
//! A node's place (pdfTeX's `sync_tag` and `sync_line`, which `get_node`
//! gives every node of `medium_node_size` or more: boxes, rules, glue,
//! kerns, math nodes) is a handle ([`Side`]) into the table of places,
//! outside the node's value. A node gets its place where it enters a
//! list, a box register or a box being made (`tail_append`, `box_end`,
//! the line breaker's lines…): the nodes the engine's algorithms make
//! have none until then, and none of these algorithms reads input, so the
//! place then is the place they were made at (the tag of the file being
//! read and TeX's `line`). Copies keep their places but rules, whose
//! places pdfTeX does not copy.
//!
//! Not done: the DVI mode (no file is written); a document that sets
//! `\synctex` with no `-synctex` on the command line (no file).

use alloc::sync::Arc;
use alloc::vec::Vec;

use partex_engine::node::{BoxNode, Node};
use partex_engine::origin::Side;

use crate::host::Host;
use crate::tex::Tex;
use crate::track::Tracker;

/// A node's place, as the machine reads it: `sync_tag`, `sync_line`.
pub type Place = (i32, i32);

/// What the engine tells the controller, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// `synctexstartinput`: a file opened, given tag `tag`, named `name`
    /// (`SYNCTEX_GET_CURRENT_NAME`), `\synctex` being `value`.
    Input {
        tag: i32,
        name: Arc<[u8]>,
        value: i32,
    },
    /// `synctexsheet(mag)`, `pages` being `total_pages`.
    Sheet { mag: i32, pages: i32, value: i32 },
    /// `synctexteehs`.
    Teehs { pages: i32 },
    /// `synctexpdfxform`: form `form` (`pdf_cur_form`) begins.
    Form { form: i32, value: i32 },
    /// `synctexmrofxfdp`.
    EndForm,
    /// `synctexpdfrefxform(objnum)` at `(h, v)`.
    RefForm { objnum: i32, h: i32, v: i32 },
    /// `synctexhlist`, `synctexvlist`: box `id` (the walk's numbering of
    /// nodes) at `(h, v)`, its place and dimensions.
    Box {
        vertical: bool,
        id: u32,
        sync: Side,
        h: i32,
        v: i32,
        dims: [i32; 3],
    },
    /// `synctextsilh`, `synctextsilv`.
    EndBox {
        vertical: bool,
        id: u32,
        sync: Side,
        h: i32,
        v: i32,
    },
    /// `synctexvoidhlist`, `synctexvoidvlist`: an empty box.
    Void {
        vertical: bool,
        id: u32,
        sync: Side,
        h: i32,
        v: i32,
        dims: [i32; 3],
    },
    /// `synctexcurrent`: a run of characters ended at `(h, v)`.
    Current { h: i32, v: i32 },
    /// `synctexkern(p, this_box)`.
    Kern {
        id: u32,
        sync: Side,
        width: i32,
        this_box: u32,
    },
    /// `synctexmath(p, this_box)` at `(h, v)`.
    Math { id: u32, sync: Side, h: i32, v: i32 },
    /// `synctexhorizontalruleorglue(p, this_box)` of a glue node.
    Glue { id: u32, sync: Side, h: i32, v: i32 },
    /// The same of glue e-TeX's `hlist_out` made a kern of `width` (glue
    /// stretched or shrunk with the box: "Handle a glue node for mixed
    /// direction typesetting").
    GlueKern {
        id: u32,
        sync: Side,
        h: i32,
        v: i32,
        width: i32,
    },
    /// The same of a rule node: `rule_wd`, `rule_ht` and `rule_dp` then.
    Rule {
        id: u32,
        sync: Side,
        h: i32,
        v: i32,
        dims: [i32; 3],
    },
}

/// The kern whose record waits (`synctex_ctxt.recorder`, its node).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Pending {
    place: Place,
    width: i32,
}

/// `synctex.c`'s controller: its context, flags and file.
#[derive(Clone, Debug)]
pub struct Machine {
    /// `synctex_options` as the command line gave it.
    cli: i32,
    off: bool,
    no_gz: bool,
    options: i32,
    option_read: bool,
    content_ready: bool,
    not_void: bool,
    warned: bool,
    quoted: bool,
    /// `SYNCTEX_FILE`: the file is open (its text is `out`).
    open: bool,
    root_name: Option<Arc<[u8]>>,
    count: i32,
    node: u32,
    recorder: Option<Pending>,
    tag: i32,
    line: i32,
    curh: i32,
    curv: i32,
    magnification: i32,
    unit: i32,
    total_length: i32,
    lastv: i32,
    form_depth: i32,
    /// `SYNCTEX_VALUE` (`\synctex`) where the events were made.
    value: i32,
    /// The job's name (`SYNCTEX_GET_JOB_NAME`), once there is one.
    pub job: Option<Vec<u8>>,
    /// The text written.
    out: Vec<u8>,
    /// Lines for the terminal (pdfTeX `printf`s them), in order.
    pub term: Vec<Vec<u8>>,
}

/// What `synctexterminate` leaves: the text to write (none: remove the
/// files), whether it is gzipped, and whether its name ends `.gz`.
pub struct Finish {
    pub text: Option<Vec<u8>>,
    pub gzip: bool,
    pub gz_name: bool,
}

/// `fprintf`'s count of what it wrote.
fn w(out: &mut Vec<u8>, args: core::fmt::Arguments<'_>) -> i32 {
    struct V<'a>(&'a mut Vec<u8>);
    impl core::fmt::Write for V<'_> {
        fn write_str(&mut self, s: &str) -> core::fmt::Result {
            self.0.extend_from_slice(s.as_bytes());
            Ok(())
        }
    }
    let n = out.len();
    let _ = core::fmt::Write::write_fmt(&mut V(out), args);
    i32::try_from(out.len() - n).unwrap_or(i32::MAX)
}

impl Machine {
    /// The controller for command line option `cli` (`-synctex=N`).
    #[must_use]
    pub fn new(cli: i32) -> Machine {
        Machine {
            cli,
            off: false,
            no_gz: false,
            options: 0,
            option_read: false,
            content_ready: false,
            not_void: false,
            warned: false,
            quoted: false,
            open: false,
            root_name: None,
            count: 0,
            node: 0,
            recorder: None,
            tag: 0,
            line: 0,
            curh: 0,
            curv: 0,
            magnification: 0,
            unit: 1,
            total_length: 0,
            lastv: -1,
            form_depth: 0,
            value: 0,
            job: None,
            out: Vec::new(),
            term: Vec::new(),
        }
    }

    /// `_synctex_read_command_line_option`: what `\synctex` becomes, the
    /// first time (`None`: left as it is).
    pub fn read_option(&mut self) -> Option<i32> {
        if self.option_read {
            return None;
        }
        self.option_read = true;
        Some(if self.cli == 0 {
            self.off = true;
            0
        } else {
            self.no_gz = self.cli < 0;
            self.options = self.cli.saturating_abs();
            self.cli | 1
        })
    }

    /// `SYNCTEX_IS_OFF`.
    #[must_use]
    pub fn is_off(&self) -> bool {
        self.off
    }

    /// `synctexabort`.
    pub fn abort(&mut self) {
        if self.open {
            self.open = false;
            self.out = Vec::new();
        }
        self.root_name = None;
        self.off = true;
    }

    fn with_forms(&self) -> bool {
        self.options & 4 != 0
    }

    /// `SYNCTEX_SHOULD_COMPRESS_V`.
    fn compress_v(&self) -> bool {
        self.options & 8 != 0 && self.lastv == self.curv
    }

    /// `synctex_dot_open`.
    fn dot_open(&mut self) -> bool {
        if self.off || self.value == 0 {
            return false;
        }
        if self.open {
            return true;
        }
        self.read_option();
        let job = self.job.clone().unwrap_or_default();
        if job.is_empty() {
            self.term
                .push(b"\nSyncTeX information: no synchronization with keyboard input\n".to_vec());
            self.abort();
            return false;
        }
        self.quoted = job.len() > 1 && job[0] == b'"' && job[job.len() - 1] == b'"';
        self.open = true;
        self.out = Vec::new();
        // (`synctex_record_preamble`)
        let v = self.options.max(1);
        self.total_length = w(&mut self.out, format_args!("SyncTeX Version:{v}\n"));
        if self.magnification == 0 {
            self.magnification = 1000;
        }
        self.unit = 1;
        if let Some(root) = self.root_name.take() {
            self.record_input(1, &root);
        }
        self.count = 0;
        true
    }

    /// `synctex_prepare_content`.
    fn prepare_content(&mut self) -> bool {
        if self.content_ready {
            return self.open;
        }
        if self.dot_open() {
            // (`synctex_record_settings`: pdfTeX's PDF mode, offsets 0)
            let n = w(
                &mut self.out,
                format_args!(
                    "Output:pdf\nMagnification:{}\nUnit:{}\nX Offset:0\nY Offset:0\n",
                    self.magnification, self.unit
                ),
            );
            self.total_length += n;
            let n = w(&mut self.out, format_args!("Content:\n"));
            self.total_length += n;
            self.content_ready = true;
            return true;
        }
        self.abort();
        false
    }

    /// Feed event `e`; `place` gives a node's place by its handle.
    pub fn feed(&mut self, e: &Event, place: &mut dyn FnMut(Side) -> Place) {
        match e {
            Event::Input { tag, name, value } => {
                self.value = *value;
                self.start_input(*tag, name);
            }
            Event::Sheet { mag, pages, value } => {
                self.value = *value;
                if self.warn_off() {
                    return;
                }
                if *pages == 0 && *mag > 0 {
                    self.magnification = *mag;
                }
                if self.prepare_content() {
                    self.record_anchor();
                    let n = w(&mut self.out, format_args!("{{{}\n", pages + 1));
                    self.total_length += n;
                    self.count += 1;
                }
            }
            Event::Teehs { pages } => {
                if !self.off && self.open {
                    self.record_anchor();
                    let n = w(&mut self.out, format_args!("}}{pages}\n"));
                    self.total_length += n;
                    self.count += 1;
                }
            }
            Event::Form { form, value } => {
                self.value = *value;
                if self.warn_off() {
                    return;
                }
                // (`synctex_record_pdfxform`)
                if self.prepare_content() && !self.ignore_box() {
                    self.form_depth += 1;
                    if self.with_forms() {
                        let n = w(&mut self.out, format_args!("<{form}\n"));
                        self.total_length += n;
                        self.count += 1;
                    }
                }
            }
            Event::EndForm => {
                if self.open {
                    self.record_anchor();
                    self.form_depth -= 1;
                    if self.with_forms() {
                        let n = w(&mut self.out, format_args!(">\n"));
                        self.total_length += n;
                        self.count += 1;
                    }
                }
            }
            Event::RefForm { objnum, h, v } => {
                if self.open {
                    self.curh = *h;
                    self.curv = *v;
                    if !self.ignore_box() {
                        let u = self.unit;
                        let n = if self.compress_v() {
                            w(&mut self.out, format_args!("f{objnum}:{},=\n", h / u))
                        } else {
                            self.lastv = *v;
                            w(
                                &mut self.out,
                                format_args!("f{objnum}:{},{}\n", h / u, v / u),
                            )
                        };
                        self.total_length += n;
                        self.count += 1;
                    }
                }
            }
            Event::Box {
                vertical,
                id,
                sync,
                h,
                v,
                dims,
            } => {
                if self.ignore_box() {
                    return;
                }
                self.node = *id;
                self.recorder = None;
                (self.tag, self.line) = place(*sync);
                self.curh = *h;
                self.curv = *v;
                self.not_void = true;
                self.record_box(if *vertical { '[' } else { '(' }, *dims);
            }
            Event::EndBox {
                vertical,
                id,
                sync,
                h,
                v,
            } => {
                if self.ignore_box() {
                    return;
                }
                self.node = *id;
                (self.tag, self.line) = place(*sync);
                self.curh = *h;
                self.curv = *v;
                self.recorder = None;
                let n = w(
                    &mut self.out,
                    format_args!("{}\n", if *vertical { ']' } else { ')' }),
                );
                self.total_length += n;
                self.count += 1;
            }
            Event::Void {
                vertical,
                id,
                sync,
                h,
                v,
                dims,
            } => {
                if self.ignore_box() {
                    return;
                }
                if !*vertical && let Some(r) = self.recorder {
                    self.record_kern(r);
                }
                self.node = *id;
                (self.tag, self.line) = place(*sync);
                self.curh = *h;
                self.curv = *v;
                self.recorder = None;
                self.record_box(if *vertical { 'v' } else { 'h' }, *dims);
            }
            Event::Current { h, v } => {
                if self.off || self.value == 0 || !self.open {
                    return;
                }
                let (tag, line, u) = (self.tag, self.line, self.unit);
                // (compressed if the context's point is at `lastv`; the
                // point printed is the walk's)
                let n = if self.compress_v() {
                    w(&mut self.out, format_args!("x{tag},{line}:{},=\n", h / u))
                } else {
                    self.lastv = *v;
                    w(
                        &mut self.out,
                        format_args!("x{tag},{line}:{},{}\n", h / u, v / u),
                    )
                };
                self.total_length += n;
            }
            Event::Kern {
                id,
                sync,
                width,
                this_box,
            } => {
                let p = place(*sync);
                if self.ignore_node(p) {
                    return;
                }
                let k = Pending {
                    place: p,
                    width: *width,
                };
                if self.did_change(p) {
                    if let Some(r) = self.recorder {
                        self.record_kern(r);
                    }
                    let first = self.node == *this_box;
                    self.node = *id;
                    (self.tag, self.line) = p;
                    if first {
                        self.recorder = Some(k);
                    } else {
                        self.recorder = None;
                        self.record_kern(k);
                    }
                } else {
                    self.node = *id;
                    (self.tag, self.line) = p;
                    self.recorder = Some(k);
                }
            }
            Event::Math { id, sync, h, v } => {
                // (`SYNCTEX_IGNORE(p)` here is the boxes')
                if self.ignore_box() {
                    return;
                }
                let p = place(*sync);
                if let Some(r) = self.recorder
                    && self.did_change(p)
                {
                    self.record_kern(r);
                }
                self.node = *id;
                (self.tag, self.line) = p;
                self.curh = *h;
                self.curv = *v;
                self.recorder = None;
                self.record_point('$', p);
            }
            Event::Glue { id, sync, h, v } => {
                let p = place(*sync);
                if self.ignore_node(p) {
                    return;
                }
                self.node = *id;
                self.curh = *h;
                self.curv = *v;
                self.recorder = None;
                (self.tag, self.line) = p;
                self.record_point('g', p);
            }
            Event::GlueKern {
                id,
                sync,
                h,
                v,
                width,
            } => {
                let p = place(*sync);
                if self.ignore_node(p) {
                    return;
                }
                self.node = *id;
                self.curh = *h;
                self.curv = *v;
                self.recorder = None;
                (self.tag, self.line) = p;
                self.record_kern(Pending {
                    place: p,
                    width: *width,
                });
            }
            Event::Rule {
                id,
                sync,
                h,
                v,
                dims,
            } => {
                let p = place(*sync);
                if self.ignore_node(p) {
                    return;
                }
                self.node = *id;
                self.curh = *h;
                self.curv = *v;
                self.recorder = None;
                (self.tag, self.line) = p;
                if !self.open {
                    self.abort();
                    return;
                }
                let ((t, l), [wd, ht, dp], u) = (p, *dims, self.unit);
                let n = if self.compress_v() {
                    w(
                        &mut self.out,
                        format_args!("r{t},{l}:{},=:{},{},{}\n", h / u, wd / u, ht / u, dp / u),
                    )
                } else {
                    self.lastv = self.curv;
                    w(
                        &mut self.out,
                        format_args!(
                            "r{t},{l}:{},{}:{},{},{}\n",
                            h / u,
                            v / u,
                            wd / u,
                            ht / u,
                            dp / u
                        ),
                    )
                };
                self.total_length += n;
                self.count += 1;
            }
        }
    }

    /// `synctexsheet`'s and `synctexpdfxform`'s start: off for good, with
    /// a warning, once, if `\synctex` says otherwise.
    fn warn_off(&mut self) -> bool {
        if !self.off {
            return false;
        }
        if self.value != 0 && !self.warned {
            self.warned = true;
            self.term.push(
                b"\nSyncTeX warning: Synchronization was disabled from\nthe command line with -synctex=0\nChanging the value of \\synctex has no effect."
                    .to_vec(),
            );
        }
        true
    }

    /// `SYNCTEX_IGNORE` of the boxes, math nodes and forms.
    fn ignore_box(&self) -> bool {
        self.off || self.value == 0 || !self.open || (self.form_depth > 0 && !self.with_forms())
    }

    /// `SYNCTEX_IGNORE(NODE, TYPE)` of kerns, glue and rules.
    fn ignore_node(&self, p: Place) -> bool {
        self.off || self.value == 0 || p.0 <= 0 || p.1 <= 0
    }

    /// `SYNCTEX_CONTEXT_DID_CHANGE`.
    fn did_change(&self, p: Place) -> bool {
        self.node == 0 || p.0 != self.tag || p.1 != self.line
    }

    /// `synctexstartinput`, from its tag on.
    fn start_input(&mut self, tag: i32, name: &Arc<[u8]>) {
        if self.off {
            return;
        }
        if tag == 1 {
            self.root_name = Some(if name.is_empty() {
                Arc::from(&b"texput"[..])
            } else {
                name.clone()
            });
            return;
        }
        if self.open || self.dot_open() {
            self.record_input(tag, name);
        }
    }

    /// The records of the boxes: `kind`, the context's place and point
    /// and `dims`.
    fn record_box(&mut self, kind: char, dims: [i32; 3]) {
        let (t, l, u) = (self.tag, self.line, self.unit);
        let [wd, ht, dp] = dims;
        let n = if self.compress_v() {
            w(
                &mut self.out,
                format_args!(
                    "{kind}{t},{l}:{},=:{},{},{}\n",
                    self.curh / u,
                    wd / u,
                    ht / u,
                    dp / u
                ),
            )
        } else {
            self.lastv = self.curv;
            w(
                &mut self.out,
                format_args!(
                    "{kind}{t},{l}:{},{}:{},{},{}\n",
                    self.curh / u,
                    self.curv / u,
                    wd / u,
                    ht / u,
                    dp / u
                ),
            )
        };
        self.total_length += n;
        self.count += 1;
    }

    /// The glue's and math nodes' records: place `p` at the context's
    /// point.
    fn record_point(&mut self, kind: char, p: Place) {
        if !self.open {
            self.abort();
            return;
        }
        let ((t, l), u) = (p, self.unit);
        let n = if self.compress_v() {
            w(
                &mut self.out,
                format_args!("{kind}{t},{l}:{},=\n", self.curh / u),
            )
        } else {
            self.lastv = self.curv;
            w(
                &mut self.out,
                format_args!("{kind}{t},{l}:{},{}\n", self.curh / u, self.curv / u),
            )
        };
        self.total_length += n;
        self.count += 1;
    }

    /// `synctex_record_node_kern`: at the context's point (which a kern
    /// does not move).
    fn record_kern(&mut self, k: Pending) {
        if !self.open {
            self.abort();
            return;
        }
        let ((t, l), u) = (k.place, self.unit);
        let n = if self.compress_v() {
            w(
                &mut self.out,
                format_args!("k{t},{l}:{},=:{}\n", self.curh / u, k.width / u),
            )
        } else {
            self.lastv = self.curv;
            w(
                &mut self.out,
                format_args!(
                    "k{t},{l}:{},{}:{}\n",
                    self.curh / u,
                    self.curv / u,
                    k.width / u
                ),
            )
        };
        self.total_length += n;
        self.count += 1;
    }

    fn record_input(&mut self, tag: i32, name: &[u8]) {
        let mut n = w(&mut self.out, format_args!("Input:{tag}:"));
        self.out.extend_from_slice(name);
        self.out.push(b'\n');
        n += i32::try_from(name.len() + 1).unwrap_or(0);
        self.total_length += n;
    }

    fn record_anchor(&mut self) {
        let n = w(&mut self.out, format_args!("!{}\n", self.total_length));
        self.total_length = n;
        self.count += 1;
    }

    /// `synctexterminate`'s part on the file: its text (the postamble
    /// written), if it has one to keep; then off.
    pub fn terminate(&mut self, log_opened: bool) -> Finish {
        let keep = log_opened && self.open && self.not_void;
        let text = if keep {
            // (`synctex_record_postamble`)
            self.record_anchor();
            let n = w(&mut self.out, format_args!("Postamble:\n"));
            self.total_length += n;
            let n = w(&mut self.out, format_args!("Count:{}\n", self.count));
            self.total_length += n;
            self.record_anchor();
            let n = w(&mut self.out, format_args!("Post scriptum:\n"));
            self.total_length += n;
            Some(core::mem::take(&mut self.out))
        } else {
            None
        };
        let f = Finish {
            text,
            gzip: !self.no_gz,
            gz_name: !(self.no_gz || self.options & 2 != 0),
        };
        self.open = false;
        self.out = Vec::new();
        self.root_name = None;
        self.off = true;
        f
    }

    /// Whether the job's name was quoted (`"my file"`).
    #[must_use]
    pub fn quoted(&self) -> bool {
        self.quoted
    }
}

/// zlib's gzip file of `text` (`gzopen(name, "wb")`, `gzprintf`,
/// `gzclose`), from `deflated`, zlib's deflate of `text` at level 6 in its
/// zlib wrapper: the same deflate stream, in gzip's header (no name, time
/// 0, OS 3) and trailer.
#[must_use]
pub fn gzip(text: &[u8], deflated: &[u8]) -> Option<Vec<u8>> {
    let body = deflated.get(2..deflated.len().checked_sub(4)?)?;
    let mut out = Vec::with_capacity(body.len() + 18);
    out.extend_from_slice(&[0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 3]);
    out.extend_from_slice(body);
    out.extend_from_slice(&partex_engine::zlib::crc32(text).to_le_bytes());
    #[allow(clippy::cast_possible_truncation, reason = "gzip's ISIZE is mod 2^32")]
    let len = text.len() as u32;
    out.extend_from_slice(&len.to_le_bytes());
    Some(out)
}

/// A node's place as made: its tag and line, and the contents the line is
/// of (an index into [`SyncState::datas`]; `u32::MAX`: none).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Made {
    tag: i32,
    line: i32,
    data: u32,
}

const NOWHERE: Made = Made {
    tag: 0,
    line: 0,
    data: u32::MAX,
};

/// The places nodes' handles name.
#[derive(Clone, Debug)]
pub(crate) struct Places {
    entries: Vec<Made>,
}

impl Places {
    fn new() -> Self {
        Places {
            entries: alloc::vec![NOWHERE],
        }
    }

    /// A handle for place `p`, of the hashed range if `hashed` (SSA
    /// mode, `Side::HASHED`).
    fn push(&mut self, p: Made, hashed: bool) -> Side {
        let h = u32::try_from(self.entries.len()).unwrap_or(0) & !Side::HASHED;
        self.entries.push(p);
        Side(if hashed { h | Side::HASHED } else { h })
    }

    /// The place of handle `h` (none: tag 0).
    fn get(&self, h: Side) -> Made {
        if h.0 == Side::NONE.0 {
            return NOWHERE;
        }
        let i = (h.0 & !Side::HASHED) as usize;
        self.entries.get(i).copied().unwrap_or(NOWHERE)
    }
}

/// `SyncTeX` in the engine (`Tex::sync`).
#[derive(Clone, Debug)]
pub(crate) struct SyncState {
    pub(crate) machine: Machine,
    /// The command line's option (a machine made again for a render).
    cli: i32,
    pub(crate) places: Places,
    /// The place last made, its key (tag, line, contents' address).
    last: Option<((i32, i32, usize), Side)>,
    /// The contents places name, and their indices by address; the edit
    /// that replaced each (the new contents, and the old and new byte
    /// ranges of its runs of changed lines), and the edits taken.
    pub(crate) datas: Vec<Arc<[u8]>>,
    data_ix: alloc::collections::BTreeMap<usize, u32>,
    next: Vec<Option<(u32, Vec<[usize; 4]>)>>,
    edits_seen: usize,
    /// Where each line of a contents starts, by its index, once asked.
    starts: alloc::collections::BTreeMap<u32, Vec<usize>>,
    /// The walk's numbering of nodes, and its boxes open.
    next_id: u32,
    boxes: Vec<u32>,
    /// A run of characters is open in the walk.
    in_run: bool,
    /// The walk is in a leader box output again (its glue made kerns the
    /// first time, by e-TeX's `hlist_out`).
    again: u32,
}

impl SyncState {
    fn new(cli: i32) -> Self {
        SyncState {
            machine: Machine::new(cli),
            cli,
            places: Places::new(),
            last: None,
            datas: Vec::new(),
            data_ix: alloc::collections::BTreeMap::new(),
            next: Vec::new(),
            edits_seen: 0,
            starts: alloc::collections::BTreeMap::new(),
            next_id: 0,
            boxes: Vec::new(),
            in_run: false,
            again: 0,
        }
    }

    fn id(&mut self) -> u32 {
        self.next_id = self.next_id.wrapping_add(1).max(1);
        self.next_id
    }

    /// The index of contents `d` (made now if new).
    fn data(&mut self, d: &Arc<[u8]>) -> u32 {
        let n = u32::try_from(self.datas.len()).unwrap_or(u32::MAX);
        let ix = *self.data_ix.entry(d.as_ptr() as usize).or_insert(n);
        if ix == n {
            self.datas.push(d.clone());
            self.next.push(None);
        }
        ix
    }

    /// The sources' edits (old contents, new, the old and new byte ranges
    /// of the runs of changed lines), in order: each contents a place
    /// names that one replaced is followed by the other.
    fn take_edits(&mut self, edits: crate::ssa::Edits) {
        for (old, new, hunks) in edits {
            let Some(&d) = self.data_ix.get(&(old.as_ptr() as usize)) else {
                continue;
            };
            if self.next[d as usize].is_some() {
                continue;
            }
            let n = self.data(&new);
            if n != d {
                self.next[d as usize] = Some((n, hunks));
            }
        }
    }

    /// Where the lines of contents `d` start (a line ends at `\n`, `\r`
    /// or both, as web2c's `input_line` reads them).
    fn starts(&mut self, d: u32) -> &[usize] {
        let datas = &self.datas;
        self.starts.entry(d).or_insert_with(|| {
            let b = &datas[d as usize];
            let mut v = alloc::vec![0];
            let mut i = 0;
            while i < b.len() {
                match b[i] {
                    b'\n' => v.push(i + 1),
                    b'\r' => {
                        if b.get(i + 1) == Some(&b'\n') {
                            i += 1;
                        }
                        v.push(i + 1);
                    }
                    _ => {}
                }
                i += 1;
            }
            v
        })
    }

    /// Where the start of line `line` of contents `d` is now, and in
    /// which contents: moved through the edits since as a rebuild moves a
    /// step's input (`ssa::rebuild`'s `Edit::pos`: a position where bytes
    /// were inserted stays before them).
    fn map_start(&mut self, mut d: u32, mut line: i32) -> (u32, i32) {
        while let Some((n, hunks)) = self.next.get(d as usize).cloned().flatten() {
            let s = self.starts(d);
            let x = usize::try_from(line - 1)
                .ok()
                .and_then(|l| s.get(l).copied())
                .unwrap_or_else(|| self.datas[d as usize].len());
            let after = hunks.partition_point(|h| x > h[1] || (x == h[1] && h[1] > h[0]));
            let y = match after.checked_sub(1).map(|j| hunks[j]) {
                Some(h) => x - h[1] + h[3],
                None => x,
            };
            let s = self.starts(n);
            line = i32::try_from(s.partition_point(|&a| a <= y)).unwrap_or(i32::MAX);
            d = n;
        }
        (d, line)
    }

    /// The line of place `p` now: moved as a rebuild moves the read of
    /// its line.
    fn map_place(&mut self, p: Made) -> i32 {
        if p.data == u32::MAX || p.line <= 0 {
            return p.line;
        }
        self.map_start(p.data, p.line).1
    }
}

/// `Tex::sync`.
pub(crate) type State = Option<alloc::boxed::Box<SyncState>>;

/// Whether `list` has a node `SyncTeX` places that has no place yet.
fn unplaced(list: &[Node]) -> bool {
    list.iter().any(unplaced_node)
}

fn unplaced_node(n: &Node) -> bool {
    match n {
        Node::Rule { sync, .. }
        | Node::Glue { sync, .. }
        | Node::Kern { sync, .. }
        | Node::Math { sync, .. } => sync.0 == 0,
        Node::Box(b) => b.sync.0 == 0 || unplaced(&b.list),
        Node::Leaders(l) => l.sync.0 == 0 || unplaced_node(&l.leader),
        Node::Unset(u) => u.sync.0 == 0 || unplaced(&u.list),
        Node::Disc(d) => unplaced(&d.pre) || unplaced(&d.post) || unplaced(&d.replace),
        Node::Ins(i) => unplaced(&i.list),
        Node::Adjust(a) => unplaced(&a.list),
        _ => false,
    }
}

/// Give place `s` to each node of `list` that has none, inside boxes too.
fn place_list(list: &mut [Node], s: Side) {
    for n in list {
        place_node(n, s);
    }
}

fn place_node(n: &mut Node, s: Side) {
    match n {
        Node::Rule { sync, .. }
        | Node::Glue { sync, .. }
        | Node::Kern { sync, .. }
        | Node::Math { sync, .. } => {
            if sync.0 == 0 {
                *sync = s;
            }
        }
        Node::Box(b) => {
            if b.sync.0 == 0 || unplaced(&b.list) {
                place_box(Arc::make_mut(b), s);
            }
        }
        Node::Leaders(l) => {
            if l.sync.0 == 0 {
                l.sync = s;
            }
            place_node(&mut l.leader, s);
        }
        Node::Unset(u) => {
            if u.sync.0 == 0 {
                u.sync = s;
            }
            place_list(&mut u.list, s);
        }
        Node::Disc(d) => {
            place_list(&mut d.pre, s);
            place_list(&mut d.post, s);
            place_list(&mut d.replace, s);
        }
        Node::Ins(i) => place_list(&mut i.list, s),
        Node::Adjust(a) => place_list(&mut a.list, s),
        _ => {}
    }
}

fn place_box(b: &mut BoxNode, s: Side) {
    if b.sync.0 == 0 {
        b.sync = s;
    }
    place_list(&mut b.list, s);
}

/// Give place `s` to each rule of `list` (inside boxes too): a copy's.
fn replace_rules(list: &mut [Node], s: Side) {
    for n in list {
        match n {
            Node::Rule { sync, .. } => *sync = s,
            Node::Box(b) => {
                if has_rule(&b.list) {
                    replace_rules(&mut Arc::make_mut(b).list, s);
                }
            }
            Node::Leaders(l) => replace_rules(core::slice::from_mut(&mut l.leader), s),
            Node::Unset(u) => replace_rules(&mut u.list, s),
            Node::Disc(d) => {
                replace_rules(&mut d.pre, s);
                replace_rules(&mut d.post, s);
                replace_rules(&mut d.replace, s);
            }
            Node::Ins(i) => replace_rules(&mut i.list, s),
            Node::Adjust(a) => replace_rules(&mut a.list, s),
            _ => {}
        }
    }
}

fn has_rule(list: &[Node]) -> bool {
    list.iter().any(|n| match n {
        Node::Rule { .. } => true,
        Node::Box(b) => has_rule(&b.list),
        Node::Leaders(l) => has_rule(core::slice::from_ref(&l.leader)),
        Node::Unset(u) => has_rule(&u.list),
        Node::Disc(d) => has_rule(&d.pre) || has_rule(&d.post) || has_rule(&d.replace),
        Node::Ins(i) => has_rule(&i.list),
        Node::Adjust(a) => has_rule(&a.list),
        _ => false,
    })
}

impl<H: Host, T: Tracker> Tex<H, T> {
    /// `SyncTeX` as `-synctex=option` asks (0: off for good, a warning if
    /// the document sets `\synctex`; below 0: the file not gzipped).
    pub fn set_synctex(&mut self, option: i32) {
        self.sync = Some(alloc::boxed::Box::new(SyncState::new(option)));
    }

    /// Whether `SyncTeX` was asked for.
    #[must_use]
    pub fn synctex_on(&self) -> bool {
        self.sync.is_some()
    }

    /// `synctex_init_command`: `\synctex` as the command line says, as
    /// TeX comes to life (unless the first file's open did it).
    pub(crate) fn synctex_init_command(&mut self) {
        if let Some(st) = self.sync.as_deref_mut()
            && let Some(v) = st.machine.read_option()
        {
            self.set_int_par(partex_engine::web::SYNCTEX_CODE, v);
        }
    }

    /// `synctexstartinput`: the file just opened at the current level,
    /// which the host found as `found`, gets its tag.
    pub(crate) fn synctex_start_input(&mut self, found: &[u8]) {
        self.synctex_init_command();
        let Some(st) = self.sync.as_deref_mut() else {
            return;
        };
        if st.machine.is_off() {
            return;
        }
        let tag = self.synctex_tags() + 1;
        self.set_synctex_tags(tag);
        if let Some(Some(f)) = self.input_file.get_mut(self.in_open) {
            f.synctex_tag = tag;
        }
        let name: Arc<[u8]> = self.host.synctex_name(found).into();
        let value = self.int_par(partex_engine::web::SYNCTEX_CODE);
        self.synctex_event(Event::Input { tag, name, value });
    }

    /// Event `e` for the controller: fed to it now, or, in SSA mode, an
    /// effect of the step (rendered by [`Tex::synctex_file`]).
    fn synctex_event(&mut self, e: Event) {
        if T::VALUES
            && let Some(fx) = &mut self.effects
        {
            if let Some(crate::effects::Effect::Synctex(v)) = fx.last_mut() {
                v.push(e);
            } else {
                fx.push(crate::effects::Effect::Synctex(alloc::vec![e]));
            }
            return;
        }
        let e = &e;
        let Some(st) = self.sync.as_deref_mut() else {
            return;
        };
        if st.machine.job.is_none()
            && matches!(
                e,
                Event::Input { .. } | Event::Sheet { .. } | Event::Form { .. }
            )
        {
            let job = self.job_name_bytes();
            if let Some(st) = self.sync.as_deref_mut() {
                st.machine.job = job;
            }
        }
        let Some(st) = self.sync.as_deref_mut() else {
            return;
        };
        let places = &st.places;
        st.machine.feed(e, &mut |h| {
            let p = places.get(h);
            (p.tag, p.line)
        });
        if !st.machine.term.is_empty() {
            let lines = core::mem::take(&mut st.machine.term);
            for l in lines {
                self.term_bytes(&l);
            }
        }
    }

    /// The place of a node made now: the tag of the file being read
    /// (`synctex_tag`, the innermost file level's) and TeX's `line`.
    #[inline(never)]
    fn sync_here(&mut self) -> Side {
        let (tag, data) = match self.input_file.get(self.in_open) {
            Some(Some(f)) => (f.synctex_tag, Some(&f.data)),
            _ => (0, None),
        };
        let line = self.line;
        let addr = data.map_or(0, |d| d.as_ptr() as usize);
        let Some(st) = self.sync.as_deref_mut() else {
            return Side(0);
        };
        if let Some((k, h)) = st.last
            && k == (tag, line, addr)
        {
            return h;
        }
        let d = match data {
            Some(d) if tag > 0 => st.data(d),
            _ => u32::MAX,
        };
        // (in SSA mode each place is a handle of its own, hashed: a node
        // made again is another value)
        let h = st.places.push(Made { tag, line, data: d }, T::VALUES);
        if !T::VALUES {
            st.last = Some(((tag, line, addr), h));
        }
        h
    }

    /// Give the nodes of `list` that have no place the place now.
    #[inline]
    pub(crate) fn sync_list(&mut self, list: &mut [Node]) {
        if self.sync.is_some() && unplaced(list) {
            let s = self.sync_here();
            place_list(list, s);
        }
    }

    /// [`Tex::sync_list`] of one node.
    #[inline]
    pub(crate) fn sync_node(&mut self, n: &mut Node) {
        if self.sync.is_some() && unplaced_node(n) {
            let s = self.sync_here();
            place_node(n, s);
        }
    }

    /// [`Tex::sync_list`] of a box in a register.
    #[inline]
    pub(crate) fn sync_arc(&mut self, b: &mut Arc<BoxNode>) {
        if self.sync.is_some() && (b.sync.0 == 0 || unplaced(&b.list)) {
            let s = self.sync_here();
            place_box(Arc::make_mut(b), s);
        }
    }

    /// [`Tex::sync_list`] of a box being made.
    #[inline]
    pub(crate) fn sync_box(&mut self, b: &mut BoxNode) {
        if self.sync.is_some() && (b.sync.0 == 0 || unplaced(&b.list)) {
            let s = self.sync_here();
            place_box(b, s);
        }
    }

    /// A copy of `list` (`\copy`, `\unhcopy`, `\unvcopy`): its rules made
    /// now (pdfTeX copies every node's place but a rule's).
    #[inline]
    pub(crate) fn sync_copied(&mut self, list: &mut [Node]) {
        if self.sync.is_some() && has_rule(list) {
            let s = self.sync_here();
            replace_rules(list, s);
        }
    }

    // ---- the ship's events ----

    /// `synctexsheet(mag)` or `synctexpdfxform(p)`: a ship begins.
    pub(crate) fn synctex_ship_begin(&mut self, shipping_page: bool) {
        let Some(st) = self.sync.as_deref_mut() else {
            return;
        };
        st.in_run = false;
        st.boxes.clear();
        st.again = 0;
        let value = self.int_par(partex_engine::web::SYNCTEX_CODE);
        let e = if shipping_page {
            Event::Sheet {
                mag: self.int_par(partex_engine::web::MAG_CODE),
                pages: self.pdf.ship.total_pages,
                value,
            }
        } else {
            Event::Form {
                form: self.pdf.ship.cur_form,
                value,
            }
        };
        self.synctex_event(e);
    }

    /// `synctexteehs` or `synctexmrofxfdp`: the ship ends.
    pub(crate) fn synctex_ship_end(&mut self, shipping_page: bool) {
        if self.sync.is_none() {
            return;
        }
        let e = if shipping_page {
            Event::Teehs {
                pages: self.pdf.ship.total_pages,
            }
        } else {
            Event::EndForm
        };
        self.synctex_event(e);
    }

    /// A box's list begins in the walk (`synctexhlist`, `synctexvlist`) at
    /// the current point.
    pub(crate) fn synctex_box(&mut self, b: &BoxNode) {
        let Some(st) = self.sync.as_deref_mut() else {
            return;
        };
        let id = st.id();
        st.boxes.push(id);
        st.in_run = false;
        let (h, v) = (self.pdf.ship.cur_h, self.pdf.ship.cur_v);
        self.synctex_event(Event::Box {
            vertical: b.vertical,
            id,
            sync: b.sync,
            h,
            v,
            dims: [b.width, b.height, b.depth],
        });
    }

    /// A box's list ends in the walk (`synctextsilh`, `synctextsilv`); a
    /// run of characters at its end ends with it (`synctexcurrent`).
    pub(crate) fn synctex_box_end(&mut self, b: &BoxNode) {
        let Some(st) = self.sync.as_deref_mut() else {
            return;
        };
        let run = core::mem::replace(&mut st.in_run, false);
        let id = st.boxes.pop().unwrap_or(0);
        let (h, v) = (self.pdf.ship.cur_h, self.pdf.ship.cur_v);
        if run {
            self.synctex_event(Event::Current { h, v });
        }
        self.synctex_event(Event::EndBox {
            vertical: b.vertical,
            id,
            sync: b.sync,
            h,
            v,
        });
    }

    /// The walk is at node `p` of an hlist: a run of characters ends at a
    /// node that is not one (`synctexcurrent`); a ligature begins one,
    /// which goes on over the characters after it.
    pub(crate) fn synctex_hnode(&mut self, p: &Node) {
        let Some(st) = self.sync.as_deref_mut() else {
            return;
        };
        let chars = matches!(p, Node::Glyphs(_));
        let ended = st.in_run && !chars;
        st.in_run = chars || matches!(p, Node::Ligature(_));
        if ended {
            let (h, v) = (self.pdf.ship.cur_h, self.pdf.ship.cur_v);
            self.synctex_event(Event::Current { h, v });
        }
    }

    /// An empty box in a list (`synctexvoidhlist`, `synctexvoidvlist`).
    pub(crate) fn synctex_void(&mut self, b: &BoxNode) {
        let Some(st) = self.sync.as_deref_mut() else {
            return;
        };
        let id = st.id();
        let (h, v) = (self.pdf.ship.cur_h, self.pdf.ship.cur_v);
        self.synctex_event(Event::Void {
            vertical: b.vertical,
            id,
            sync: b.sync,
            h,
            v,
            dims: [b.width, b.height, b.depth],
        });
    }

    /// A kern in an hlist (`synctexkern`), before the walk moves past it.
    pub(crate) fn synctex_kern(&mut self, sync: Side, width: i32) {
        let Some(st) = self.sync.as_deref_mut() else {
            return;
        };
        let id = st.id();
        let this_box = st.boxes.last().copied().unwrap_or(0);
        self.synctex_event(Event::Kern {
            id,
            sync,
            width,
            this_box,
        });
    }

    /// A math node in an hlist (`synctexmath`), before the walk moves
    /// past it.
    pub(crate) fn synctex_math(&mut self, sync: Side) {
        let Some(st) = self.sync.as_deref_mut() else {
            return;
        };
        let id = st.id();
        let (h, v) = (self.pdf.ship.cur_h, self.pdf.ship.cur_v);
        self.synctex_event(Event::Math { id, sync, h, v });
    }

    /// Glue in an hlist, moved past (`synctexhorizontalruleorglue`).
    pub(crate) fn synctex_glue(&mut self, sync: Side) {
        let Some(st) = self.sync.as_deref_mut() else {
            return;
        };
        let id = st.id();
        let (h, v) = (self.pdf.ship.cur_h, self.pdf.ship.cur_v);
        self.synctex_event(Event::Glue { id, sync, h, v });
    }

    /// Glue `sync` in an hlist moved past by `width`: with e-TeX's mode,
    /// glue set with the box (`converts`) is a kern then, and recorded as
    /// one (by `synctexkern`, in a leader box output again: a kern node
    /// since the first time).
    pub(crate) fn synctex_glue_moved(&mut self, sync: Side, width: i32, converts: bool) {
        let Some(st) = self.sync.as_deref_mut() else {
            return;
        };
        if !converts {
            self.synctex_glue(sync);
        } else if st.again > 0 {
            self.synctex_kern(sync, width);
        } else {
            let id = st.id();
            let (h, v) = (self.pdf.ship.cur_h, self.pdf.ship.cur_v);
            self.synctex_event(Event::GlueKern {
                id,
                sync,
                h,
                v,
                width,
            });
        }
    }

    /// A leader box is output again (`on`), or that ends.
    pub(crate) fn synctex_again(&mut self, on: bool) {
        if let Some(st) = self.sync.as_deref_mut() {
            if on {
                st.again += 1;
            } else {
                st.again = st.again.saturating_sub(1);
            }
        }
    }

    /// A rule in an hlist, moved past: `rule_wd`, `rule_ht` (the
    /// thickness) and `rule_dp` as they are then.
    pub(crate) fn synctex_rule(&mut self, sync: Side, dims: [i32; 3]) {
        let Some(st) = self.sync.as_deref_mut() else {
            return;
        };
        let id = st.id();
        let (h, v) = (self.pdf.ship.cur_h, self.pdf.ship.cur_v);
        self.synctex_event(Event::Rule {
            id,
            sync,
            h,
            v,
            dims,
        });
    }

    /// A form drawn (`out_form`'s `synctexpdfrefxform`).
    pub(crate) fn synctex_refxform(&mut self, objnum: i32) {
        if self.sync.is_none() {
            return;
        }
        let (h, v) = (self.pdf.ship.cur_h, self.pdf.ship.cur_v);
        self.synctex_event(Event::RefForm { objnum, h, v });
    }

    /// `synctexabort` (the PDF file not written: a fatal error).
    pub(crate) fn synctex_abort(&mut self) {
        if let Some(st) = self.sync.as_deref_mut() {
            st.machine.abort();
        }
    }

    /// `synctexterminate(log_opened)`: the file written, gzipped as zlib
    /// writes it, `SyncTeX written on NAME.` on the terminal (not in
    /// batch mode), and the files of earlier runs it replaces removed.
    /// In SSA mode the file is the build's ([`Tex::synctex_write`], once
    /// it is linked): only the message, from the steps' events so far.
    pub(crate) fn synctex_terminate(&mut self, log_opened: bool) {
        if self.sync.is_none() {
            return;
        }
        let (f, quoted) = if T::VALUES {
            match self.synctex_render(log_opened) {
                Some(r) => r,
                None => return,
            }
        } else {
            let Some(st) = self.sync.as_deref_mut() else {
                return;
            };
            (st.machine.terminate(log_opened), st.machine.quoted())
        };
        let Some((name, other)) = self.synctex_names(&f) else {
            return;
        };
        let printed = if T::VALUES {
            if f.text.is_none() {
                return;
            }
            self.host.output_name(&name, crate::host::FileKind::Other)
        } else {
            let Some(bytes) = self.synctex_bytes(f) else {
                self.host.remove_output(&name);
                self.host.remove_output(&other);
                return;
            };
            self.host.remove_output(&other);
            let Some((id, printed)) = self.open_out(&name, crate::host::FileKind::Other) else {
                return;
            };
            self.out_write(id, &bytes);
            self.out_close(id);
            printed
        };
        if self.interaction() > crate::web::BATCH_MODE {
            let mut m = b"\nSyncTeX written on ".to_vec();
            if quoted {
                m.push(b'"');
                m.extend_from_slice(&printed);
                m.push(b'"');
            } else {
                m.extend_from_slice(&printed);
                m.push(b'.');
            }
            self.term_bytes(&m);
        }
    }

    /// The file's name as `f` has it, and the other one's (`.gz` or not),
    /// from the log's name (the job's, unquoted, without its extension).
    fn synctex_names(&self, f: &Finish) -> Option<(Vec<u8>, Vec<u8>)> {
        let job = self.job_name_bytes()?;
        let mut base: Vec<u8> = job.into_iter().filter(|&c| c != b'"').collect();
        base.extend_from_slice(b".synctex");
        let mut gz = base.clone();
        gz.extend_from_slice(b".gz");
        Some(if f.gz_name { (gz, base) } else { (base, gz) })
    }

    /// The file's bytes (none: no file), gzipped if `f` says so.
    fn synctex_bytes(&mut self, f: Finish) -> Option<Vec<u8>> {
        let text = f.text?;
        if f.gzip {
            self.host.deflate(6, &text).and_then(|d| gzip(&text, &d))
        } else {
            Some(text)
        }
    }

    /// SSA mode: every step's events so far, in order, the places' lines
    /// moved through the edits since they were made, fed to a controller
    /// made for them: what `synctexterminate` leaves, and whether the
    /// job's name was quoted.
    fn synctex_render(&mut self, log_opened: bool) -> Option<(Finish, bool)> {
        let job = self.job_name_bytes();
        let st = self.sync.as_deref_mut()?;
        let rec = self.tracker.ssa()?.rec.borrow();
        let edits = crate::ssa::edits_from(&rec, st.edits_seen);
        st.edits_seen += edits.len();
        st.take_edits(edits);
        let chunks = crate::ssa::step_effects(&rec);
        drop(rec);
        let mut m = Machine::new(st.cli);
        m.read_option();
        m.job = job;
        let mut seen: alloc::collections::BTreeMap<u32, Place> =
            alloc::collections::BTreeMap::new();
        let mut place = |h: Side| {
            *seen.entry(h.0).or_insert_with(|| {
                let p = st.places.get(h);
                (p.tag, st.map_place(p))
            })
        };
        let pending = self.effects.as_deref().unwrap_or_default();
        let effects = chunks
            .iter()
            .flat_map(|(_, c)| c.1.iter())
            .chain(pending.iter());
        for e in effects {
            if let crate::effects::Effect::Synctex(v) = e {
                for ev in v {
                    m.feed(ev, &mut place);
                }
            }
        }
        let f = m.terminate(log_opened);
        Some((f, m.quoted()))
    }

    /// SSA mode: write the build's `SyncTeX` file (its steps' events
    /// rendered; none: the files of earlier runs removed), after the
    /// build is linked. Nothing in a plain run, whose file is written at
    /// its end.
    pub fn synctex_write(&mut self) {
        if !T::VALUES || self.sync.is_none() {
            return;
        }
        let log_opened = self.log_opened;
        let Some((f, _)) = self.synctex_render(log_opened) else {
            return;
        };
        let Some((name, other)) = self.synctex_names(&f) else {
            return;
        };
        self.host.remove_output(&other);
        let Some(bytes) = self.synctex_bytes(f) else {
            self.host.remove_output(&name);
            return;
        };
        if let Some((id, _)) = self.host.open_write(&name, crate::host::FileKind::Other) {
            self.host.write(id, &bytes);
            self.host.close(id);
        }
    }
}

// (events outlive the process in a persisted build, their places not)
partex_engine::persist_enum!(Event {
    Input { tag, name, value },
    Sheet { mag, pages, value },
    Teehs { pages },
    Form { form, value },
    EndForm,
    RefForm { objnum, h, v },
    Box {
        vertical,
        id,
        sync,
        h,
        v,
        dims
    },
    EndBox {
        vertical,
        id,
        sync,
        h,
        v
    },
    Void {
        vertical,
        id,
        sync,
        h,
        v,
        dims
    },
    Current { h, v },
    Kern {
        id,
        sync,
        width,
        this_box
    },
    Math { id, sync, h, v },
    Glue { id, sync, h, v },
    GlueKern {
        id,
        sync,
        h,
        v,
        width
    },
    Rule {
        id,
        sync,
        h,
        v,
        dims
    },
});
