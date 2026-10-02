//! Getting the top-level `.aux` file name (§97–108) and reading the
//! `.aux` files (§109–145).

use alloc::vec::Vec;

use crate::table::{
    AUX_COMMAND_ILK, AUX_FILE_ILK, BIB_FILE_ILK, BST_FILE_ILK, CITE_ILK, LC_CITE_ILK, Str, TEXT_ILK,
};
use crate::{AuxFrame, Bib, Ch, Piece, R, Reader, Src, WHITE_SPACE, lex};

// §78
pub const N_AUX_BIBDATA: i64 = 0;
pub const N_AUX_BIBSTYLE: i64 = 1;
pub const N_AUX_CITATION: i64 = 2;
pub const N_AUX_INPUT: i64 = 3;

/// §111: an error in an `.aux` command; the rest of the line is skipped.
struct AuxErr;

impl Bib<'_> {
    /// §100–107 (bibtex.ch): open the `.aux` file named on the command
    /// line; the names of the `.blg` and `.bbl` files, or `None` (after
    /// telling the terminal) if it can't be read.
    pub(crate) fn get_the_top_level_aux_file_name(
        &mut self,
        arg: &[u8],
    ) -> Option<(Vec<u8>, Vec<u8>)> {
        // §106: don't add the extension if it's already there
        let stem = if arg.len() >= 4 && arg.ends_with(b".aux") {
            &arg[..arg.len() - 4]
        } else {
            arg
        };
        let mut aux_name = stem.to_vec();
        aux_name.extend_from_slice(b".aux");
        let Some(data) = self.files.aux(&aux_name) else {
            // §99
            self.term.extend_from_slice(b"I couldn't open file name `");
            self.term.extend_from_slice(&aux_name);
            self.term.extend_from_slice(b"'\n");
            return None;
        };
        let with = |ext: &[u8]| {
            let mut n = stem.to_vec();
            n.extend_from_slice(ext);
            n
        };
        // §107
        let (loc, _) = self.t.insert(TEXT_ILK, stem);
        self.top_lev_str = self.t.text(loc).clone();
        let (loc, _) = self.t.insert(AUX_FILE_ILK, &aux_name);
        let name = self.t.text(loc).clone();
        self.aux.push(AuxFrame {
            file: Reader::new(data),
            name,
            line: 0,
        });
        Some((with(b".blg"), with(b".bbl")))
    }

    fn cur_aux_name(&self) -> Str {
        self.aux
            .last()
            .map_or_else(|| self.s_null.clone(), |f| f.name.clone())
    }

    /// §108
    fn print_aux_name(&mut self) {
        let s = self.cur_aux_name();
        self.print_ln(&[&s]);
    }

    /// §110
    pub(crate) fn read_aux(&mut self) -> R {
        let s = self.cur_aux_name();
        self.progress(&[&"The top-level auxiliary file: ", &s]);
        loop {
            self.aux.last_mut().expect("an open .aux file").line += 1;
            if self.input_ln(Src::Aux) {
                self.get_aux_command_and_process()?;
            } else if self.aux.len() == 1 {
                // §142: `pop_the_aux_stack` at the top level ends the loop
                break;
            } else {
                self.aux.pop();
            }
        }
        self.last_check_for_aux_errors();
        Ok(())
    }

    /// §111: `aux_err_print`.
    fn aux_err_print(&mut self) -> AuxErr {
        let line = self.aux.last().map_or(0, |f| f.line);
        self.print(&[&"---line ", &line, &" of file "]);
        self.print_aux_name();
        self.print_bad_input_line();
        self.print_skipping_whatever_remains();
        self.print_ln(&[&"command"]);
        AuxErr
    }

    /// §111: `aux_err`.
    fn aux_err(&mut self, pieces: &[&dyn Piece]) -> AuxErr {
        self.print(pieces);
        self.aux_err_print()
    }

    /// §116
    fn get_aux_command_and_process(&mut self) -> R {
        self.buf_ptr2 = 0;
        if !self.scan1(b'{') {
            return Ok(());
        }
        let Some(loc) = self.find_token(AUX_COMMAND_ILK) else {
            return Ok(());
        };
        let r = match self.t.info(loc) {
            N_AUX_BIBDATA => self.aux_bib_data_command(),
            N_AUX_BIBSTYLE => self.aux_bib_style_command(),
            N_AUX_CITATION => self.aux_citation_command(),
            N_AUX_INPUT => self.aux_input_command()?,
            _ => return Err(self.confusion("Unknown auxiliary-file command")),
        };
        let _ = r;
        Ok(())
    }

    /// §112–115: the argument checks shared by the commands; `Ok(false)`
    /// after an error.
    fn aux_arg_err(&mut self, found: bool, only_one: bool) -> Result<(), AuxErr> {
        if !found {
            return Err(self.aux_err(&[&"No \"", &Ch(b'}'), &"\""]));
        }
        if lex(self.scan_char()) == WHITE_SPACE {
            return Err(self.aux_err(&[&"White space in argument"]));
        }
        if self.last > self.buf_ptr2 + 1 && (only_one || self.scan_char() == b'}') {
            return Err(self.aux_err(&[&"Stuff after \"", &Ch(b'}'), &"\""]));
        }
        Ok(())
    }

    /// §112
    fn aux_err_illegal_another(&mut self, what: &str) -> AuxErr {
        self.aux_err(&[&"Illegal, another \\bib", &what, &" command"])
    }

    /// §120
    fn aux_bib_data_command(&mut self) -> Result<(), AuxErr> {
        if self.bib_seen {
            return Err(self.aux_err_illegal_another("data"));
        }
        self.bib_seen = true;
        while self.scan_char() != b'}' {
            self.buf_ptr2 += 1;
            let found = self.scan2_white(b'}', b',');
            self.aux_arg_err(found, false)?;
            // §123: open a .bib file
            let (loc, found) = self.insert_token(BIB_FILE_ILK);
            let name = self.t.text(loc).clone();
            crate::set(&mut self.bib_list, self.bib_ptr, name.clone());
            if found {
                self.print(&[&"This database file appears more than once: "]);
                self.print_bib_name();
                return Err(self.aux_err_print());
            }
            let Some(data) = self.files.bib(&name) else {
                self.print(&[&"I couldn't open database file "]);
                self.print_bib_name();
                return Err(self.aux_err_print());
            };
            crate::set(&mut self.bib_files, self.bib_ptr, Some(Reader::new(data)));
            self.bib_ptr += 1;
        }
        Ok(())
    }

    /// §126
    fn aux_bib_style_command(&mut self) -> Result<(), AuxErr> {
        if self.bst_seen {
            return Err(self.aux_err_illegal_another("style"));
        }
        self.bst_seen = true;
        self.buf_ptr2 += 1;
        let found = self.scan1_white(b'}');
        self.aux_arg_err(found, true)?;
        // §127: open the .bst file
        let (loc, found) = self.insert_token(BST_FILE_ILK);
        let name = self.t.text(loc).clone();
        self.bst_str = Some(name.clone());
        if found {
            // (can't happen: this is the only \bibstyle)
            self.bst_str = None;
            return Err(AuxErr);
        }
        let Some(data) = self.files.bst(&name) else {
            self.print(&[&"I couldn't open style file "]);
            self.print_bst_name();
            self.bst_str = None;
            return Err(self.aux_err_print());
        };
        self.bst_file = Reader::new(data);
        self.progress(&[&"The style file: ", &name, &".bst"]);
        Ok(())
    }

    /// §132
    fn aux_citation_command(&mut self) -> Result<(), AuxErr> {
        self.citation_seen = true;
        while self.scan_char() != b'}' {
            self.buf_ptr2 += 1;
            let found = self.scan2_white(b'}', b',');
            self.aux_arg_err(found, false)?;
            // §134: `\citation{*}` includes the entire database
            if self.token() == b"*" {
                if self.all_entries {
                    self.print_ln(&[&"Multiple inclusions of entire database"]);
                    return Err(self.aux_err_print());
                }
                self.all_entries = true;
                self.all_marker = self.cite_ptr;
                continue;
            }
            // §133
            let lc = self.token().to_ascii_lowercase();
            let (lc_cite_loc, found) = self.t.insert(LC_CITE_ILK, &lc);
            if found {
                // §135
                if self.find_token(CITE_ILK).is_none() {
                    self.print(&[&"Case mismatch error between cite keys "]);
                    self.print_token();
                    self.print(&[&" and "]);
                    let cite_loc = self.t.info(lc_cite_loc) as u32;
                    let s = self.cite_list[self.t.info(cite_loc) as usize].clone();
                    self.print(&[&s]);
                    self.print_newline();
                    return Err(self.aux_err_print());
                }
            } else {
                // §136
                let (cite_loc, _) = self.insert_token(CITE_ILK);
                let s = self.t.text(cite_loc).clone();
                crate::set(&mut self.cite_list, self.cite_ptr, s);
                self.t.set_info(cite_loc, self.cite_ptr as i64);
                self.t.set_info(lc_cite_loc, i64::from(cite_loc));
                self.cite_ptr += 1;
            }
        }
        Ok(())
    }

    /// §139; `Ok(Err(…))` after an error in the command.
    fn aux_input_command(&mut self) -> R<Result<(), AuxErr>> {
        self.buf_ptr2 += 1;
        let found = self.scan1_white(b'}');
        if let Err(e) = self.aux_arg_err(found, true) {
            return Ok(Err(e));
        }
        // §140: push the .aux stack
        if self.aux.len() == 20 {
            self.print_token();
            self.print(&[&": "]);
            return Err(self.overflow("auxiliary file depth ", 20));
        }
        if !self.token().ends_with(b".aux") {
            self.print_token();
            self.print(&[&" has a wrong extension"]);
            return Ok(Err(self.aux_err_print()));
        }
        let (loc, found) = self.insert_token(AUX_FILE_ILK);
        let name = self.t.text(loc).clone();
        if found {
            self.print(&[&"Already encountered file ", &name]);
            self.print_newline();
            return Ok(Err(self.aux_err_print()));
        }
        // §141 (bibtex.ch): as named, else beside the top-level file
        let mut data = self.files.aux(&name);
        if data.is_none() {
            let top = &self.top_lev_str;
            if let Some(slash) = top.iter().rposition(|&c| c == b'/')
                && !name.starts_with(b"/")
            {
                let dir = &top[..slash];
                if !dir.is_empty() && dir != b"." {
                    let mut p = dir.to_vec();
                    p.push(b'/');
                    p.extend_from_slice(&name);
                    data = self.files.aux(&p);
                }
            }
        }
        let Some(data) = data else {
            self.print(&[&"I couldn't open auxiliary file ", &name]);
            self.print_newline();
            return Ok(Err(self.aux_err_print()));
        };
        self.aux.push(AuxFrame {
            file: Reader::new(data),
            name: name.clone(),
            line: 0,
        });
        let level = self.aux.len() - 1;
        self.log_ln(&[&"A level-", &level, &" auxiliary file: ", &name]);
        Ok(Ok(()))
    }

    /// §144: `aux_end_err`.
    fn aux_end_err(&mut self, what: &str) {
        self.print(&[&"I found no ", &what, &"---while reading file "]);
        self.print_aux_name();
        self.mark_error();
    }

    /// §145
    fn last_check_for_aux_errors(&mut self) {
        self.num_cites = self.cite_ptr;
        self.num_bib_files = self.bib_ptr;
        if !self.citation_seen {
            self.aux_end_err("\\citation commands");
        } else if self.num_cites == 0 && !self.all_entries {
            self.aux_end_err("cite keys");
        }
        if !self.bib_seen {
            self.aux_end_err("\\bibdata command");
        } else if self.num_bib_files == 0 {
            self.aux_end_err("database files");
        }
        if !self.bst_seen {
            self.aux_end_err("\\bibstyle command");
        } else if self.bst_str.is_none() {
            self.aux_end_err("style file");
        }
    }
}
