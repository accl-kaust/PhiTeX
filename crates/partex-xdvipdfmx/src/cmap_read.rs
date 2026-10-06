//! cmap_read.c, cmap_read.h: reading a CMap file (PostScript resource).
//!
//! The file is read through C's `ifreader` buffer (`INPUT_BUF_SIZE`
//! chunks, refilled with `ifreader_read` before each token): tokens are
//! taken by `pst_get_token` from `buf[cursor..endptr]`, so where the
//! buffer ends matters as in C — keep its refill points exactly.

#![allow(non_snake_case)]

use crate::cmap::{CID_MAX, CMap, Cid};
use crate::pdffont::CidSysInfo;
use crate::prelude::*;
use crate::pst::{PstObj, pst_get_token};

/// `CMAP_PARSE_DEBUG_STR`.
pub const CMAP_PARSE_DEBUG_STR: &str = "CMap_parse:";
/// `TOKEN_LEN_MAX`.
pub const TOKEN_LEN_MAX: usize = 127;
/// `INPUT_BUF_SIZE`.
pub const INPUT_BUF_SIZE: usize = 4096;
/// `CMAP_SIG_MAX`.
pub const CMAP_SIG_MAX: usize = 64;

/// `ifreader`: `buf[cursor..endptr]` is what is buffered (C keeps a NUL
/// at `endptr`; `buf` has room for it: `max + 1` bytes).
#[derive(Debug)]
pub struct Ifreader<'a> {
    pub cursor: usize,
    pub endptr: usize,
    pub buf: Vec<u8>,
    pub max: usize,
    pub fp: &'a mut MemFile,
    pub unread: usize,
}

/// `ifreader_create`.
fn ifreader_create(fp: &mut MemFile, size: usize, bufsize: usize) -> Ifreader<'_> {
    let mut buf = vec![0u8; bufsize + 1];
    buf[0] = 0;
    Ifreader {
        cursor: 0,
        endptr: 0,
        buf,
        max: bufsize,
        fp,
        unread: size,
    }
}

/// `ifreader_read` (and `ifreader_need`): bytes available.
fn ifreader_read(reader: &mut Ifreader<'_>, size: usize) -> Result<usize> {
    let mut bytesread = 0;
    let bytesrem = reader.endptr - reader.cursor;
    if size > reader.max {
        reader.buf.resize(size + 1, 0);
        reader.max = size;
    }
    if reader.unread > 0 && bytesrem < size {
        bytesread = (reader.max - bytesrem).min(reader.unread);
        reader.buf.copy_within(reader.cursor..reader.endptr, 0);
        reader.cursor = 0;
        reader.endptr = bytesrem;
        let end = reader.endptr;
        let n = reader.fp.read_into(&mut reader.buf[end..end + bytesread]);
        if n != bytesread {
            fatal!("Reading file failed.");
        }
        reader.endptr += bytesread;
        reader.unread -= bytesread;
    }
    let e = reader.endptr;
    reader.buf[e] = 0;
    Ok(bytesread + bytesrem)
}

/// `ifreader_need`.
fn ifreader_need(reader: &mut Ifreader<'_>, size: usize) -> Result<usize> {
    ifreader_read(reader, size)
}

/// `pst_get_token(&(input->cursor), input->endptr)`.
fn get_token(input: &mut Ifreader<'_>) -> Result<Option<PstObj>> {
    pst_get_token(&input.buf[..input.endptr], &mut input.cursor)
}

/// The bytes before the first NUL (C's string functions).
fn cstr(s: &[u8]) -> &[u8] {
    match s.iter().position(|&c| c == 0) {
        Some(n) => &s[..n],
        None => s,
    }
}

/// `check_next_token`: 0 if the next token is `key`, else -1.
fn check_next_token(input: &mut Ifreader<'_>, key: &[u8]) -> Result<i32> {
    if ifreader_need(input, key.len())? == 0 {
        return Ok(-1);
    }
    let Some(token) = get_token(input)? else {
        return Ok(-1);
    };
    // (C's strcmp of a NULL string would crash: here it is a mismatch.)
    match token.pst_getSV()? {
        Some(s) if cstr(&s) == key => Ok(0),
        _ => Ok(-1),
    }
}

/// `get_coderange`: status, and fills `code_lo`, `code_hi` (at most
/// `maxlen` bytes) and `*dim`.
fn get_coderange(
    input: &mut Ifreader<'_>,
    code_lo: &mut [u8],
    code_hi: &mut [u8],
    dim: &mut i32,
    maxlen: i32,
) -> Result<i32> {
    let Some(tok1) = get_token(input)? else {
        return Ok(-1);
    };
    let Some(tok2) = get_token(input)? else {
        return Ok(-1);
    };
    if !tok1.is_string() || !tok2.is_string() {
        return Ok(-1);
    }
    let dim1 = tok1.pst_length_of()?;
    let dim2 = tok2.pst_length_of()?;
    if dim1 != dim2 || dim1 > maxlen {
        return Ok(-1);
    }
    code_lo[..dim1 as usize].copy_from_slice(&tok1.pst_data_ptr()?[..dim1 as usize]);
    code_hi[..dim2 as usize].copy_from_slice(&tok2.pst_data_ptr()?[..dim2 as usize]);
    *dim = dim1;
    Ok(0)
}

/// `do_codespacerange`.
fn do_codespacerange(cmap: &mut CMap, input: &mut Ifreader<'_>, count: i32) -> Result<i32> {
    let mut code_lo = [0u8; TOKEN_LEN_MAX];
    let mut code_hi = [0u8; TOKEN_LEN_MAX];
    let mut dim = 0;
    let mut count = count;
    while count > 0 {
        count -= 1;
        if get_coderange(
            input,
            &mut code_lo,
            &mut code_hi,
            &mut dim,
            TOKEN_LEN_MAX as i32,
        )? < 0
        {
            return Ok(-1);
        }
        let d = dim as usize;
        cmap.CMap_add_codespacerange(&code_lo[..d], &code_hi[..d]);
    }
    check_next_token(input, b"endcodespacerange")
}

/// `handle_codearray`: `code_lo[..dim]`, its last byte incremented.
///
/// bfrange: `<codeLo> <codeHi> [destCode1 destCode2 ...]`
fn handle_codearray(
    cmap: &mut CMap,
    input: &mut Ifreader<'_>,
    code_lo: &mut [u8],
    dim: i32,
    count: i32,
) -> Result<i32> {
    if dim < 1 {
        fatal!("Invalid code range.");
    }
    let d = dim as usize;
    let mut count = count;
    while count > 0 {
        count -= 1;
        let Some(tok) = get_token(input)? else {
            return Ok(-1);
        };
        if tok.is_string() {
            let len = tok.pst_length_of()? as usize;
            cmap.CMap_add_bfchar(&code_lo[..d], &tok.pst_data_ptr()?[..len]);
        } else if tok.is_mark() || !tok.is_name() {
            fatal!("{}: Invalid CMap mapping record.", CMAP_PARSE_DEBUG_STR);
        } else {
            fatal!(
                "{}: Mapping to charName not supported.",
                CMAP_PARSE_DEBUG_STR
            );
        }
        code_lo[d - 1] = code_lo[d - 1].wrapping_add(1);
    }
    check_next_token(input, b"]")
}

/// The CID of an integer token in range, as the `do_*` functions take it.
fn dst_cid(tok: &PstObj) -> Result<Option<Cid>> {
    let dst_cid = tok.pst_getIV()?;
    if dst_cid >= 0 && dst_cid <= CID_MAX {
        Ok(Some(dst_cid as Cid))
    } else {
        Ok(None)
    }
}

/// `do_notdefrange`.
fn do_notdefrange(cmap: &mut CMap, input: &mut Ifreader<'_>, count: i32) -> Result<i32> {
    let mut code_lo = [0u8; TOKEN_LEN_MAX];
    let mut code_hi = [0u8; TOKEN_LEN_MAX];
    let mut dim = 0;
    let mut count = count;
    while count > 0 {
        count -= 1;
        if ifreader_need(input, TOKEN_LEN_MAX * 3)? == 0 {
            return Ok(-1);
        }
        if get_coderange(
            input,
            &mut code_lo,
            &mut code_hi,
            &mut dim,
            TOKEN_LEN_MAX as i32,
        )? < 0
        {
            return Ok(-1);
        }
        let Some(tok) = get_token(input)? else {
            return Ok(-1);
        };
        if tok.is_integer() {
            if let Some(cid) = dst_cid(&tok)? {
                let d = dim as usize;
                cmap.CMap_add_notdefrange(&code_lo[..d], &code_hi[..d], cid);
            }
        } else {
            warn!(
                "{}: Invalid CMap mapping record. (ignored)",
                CMAP_PARSE_DEBUG_STR
            );
        }
    }
    check_next_token(input, b"endnotdefrange")
}

/// `do_bfrange`.
fn do_bfrange(cmap: &mut CMap, input: &mut Ifreader<'_>, count: i32) -> Result<i32> {
    let mut code_lo = [0u8; TOKEN_LEN_MAX];
    let mut code_hi = [0u8; TOKEN_LEN_MAX];
    let mut srcdim = 0;
    let mut count = count;
    while count > 0 {
        count -= 1;
        if ifreader_need(input, TOKEN_LEN_MAX * 3)? == 0 {
            return Ok(-1);
        }
        if get_coderange(
            input,
            &mut code_lo,
            &mut code_hi,
            &mut srcdim,
            TOKEN_LEN_MAX as i32,
        )? < 0
        {
            return Ok(-1);
        }
        let Some(tok) = get_token(input)? else {
            return Ok(-1);
        };
        let d = srcdim as usize;
        if tok.is_string() {
            let len = tok.pst_length_of()? as usize;
            cmap.CMap_add_bfrange(&code_lo[..d], &code_hi[..d], &tok.pst_data_ptr()?[..len]);
        } else if tok.is_mark() {
            let n = i32::from(code_hi[d - 1]) - i32::from(code_lo[d - 1]) + 1;
            if handle_codearray(cmap, input, &mut code_lo, srcdim, n)? < 0 {
                return Ok(-1);
            }
        } else {
            warn!(
                "{}: Invalid CMap mapping record. (ignored)",
                CMAP_PARSE_DEBUG_STR
            );
        }
    }
    check_next_token(input, b"endbfrange")
}

/// `do_cidrange`.
fn do_cidrange(cmap: &mut CMap, input: &mut Ifreader<'_>, count: i32) -> Result<i32> {
    let mut code_lo = [0u8; TOKEN_LEN_MAX];
    let mut code_hi = [0u8; TOKEN_LEN_MAX];
    let mut dim = 0;
    let mut count = count;
    while count > 0 {
        count -= 1;
        if ifreader_need(input, TOKEN_LEN_MAX * 3)? == 0 {
            return Ok(-1);
        }
        if get_coderange(
            input,
            &mut code_lo,
            &mut code_hi,
            &mut dim,
            TOKEN_LEN_MAX as i32,
        )? < 0
        {
            return Ok(-1);
        }
        let Some(tok) = get_token(input)? else {
            return Ok(-1);
        };
        if tok.is_integer() {
            if let Some(cid) = dst_cid(&tok)? {
                let d = dim as usize;
                cmap.CMap_add_cidrange(&code_lo[..d], &code_hi[..d], cid);
            }
        } else {
            warn!(
                "{}: Invalid CMap mapping record. (ignored)",
                CMAP_PARSE_DEBUG_STR
            );
        }
    }
    check_next_token(input, b"endcidrange")
}

/// Two tokens, as the `do_*char` functions read them.
fn get_two_tokens(input: &mut Ifreader<'_>) -> Result<Option<(PstObj, PstObj)>> {
    let tok1 = some!(get_token(input)?);
    let tok2 = some!(get_token(input)?);
    Ok(Some((tok1, tok2)))
}

/// The string token's bytes (`pst_data_ptr`, `pst_length_of`).
fn string_bytes(tok: &PstObj) -> Result<&[u8]> {
    let len = tok.pst_length_of()? as usize;
    Ok(&tok.pst_data_ptr()?[..len])
}

/// `do_notdefchar`.
fn do_notdefchar(cmap: &mut CMap, input: &mut Ifreader<'_>, count: i32) -> Result<i32> {
    let mut count = count;
    while count > 0 {
        count -= 1;
        if ifreader_need(input, TOKEN_LEN_MAX * 2)? == 0 {
            return Ok(-1);
        }
        let Some((tok1, tok2)) = get_two_tokens(input)? else {
            return Ok(-1);
        };
        if tok1.is_string() && tok2.is_integer() {
            if let Some(cid) = dst_cid(&tok2)? {
                cmap.CMap_add_notdefchar(string_bytes(&tok1)?, cid);
            }
        } else {
            warn!(
                "{}: Invalid CMap mapping record. (ignored)",
                CMAP_PARSE_DEBUG_STR
            );
        }
    }
    check_next_token(input, b"endnotdefchar")
}

/// `do_bfchar`.
fn do_bfchar(cmap: &mut CMap, input: &mut Ifreader<'_>, count: i32) -> Result<i32> {
    let mut count = count;
    while count > 0 {
        count -= 1;
        if ifreader_need(input, TOKEN_LEN_MAX * 2)? == 0 {
            return Ok(-1);
        }
        let Some((tok1, tok2)) = get_two_tokens(input)? else {
            return Ok(-1);
        };
        // We only support single CID font as descendant font, charName
        // should not come here.
        if tok1.is_string() && tok2.is_string() {
            cmap.CMap_add_bfchar(string_bytes(&tok1)?, string_bytes(&tok2)?);
        } else if tok2.is_name() {
            fatal!(
                "{}: Mapping to charName not supported.",
                CMAP_PARSE_DEBUG_STR
            );
        } else {
            warn!(
                "{}: Invalid CMap mapping record. (ignored)",
                CMAP_PARSE_DEBUG_STR
            );
        }
    }
    check_next_token(input, b"endbfchar")
}

/// `do_cidchar`.
fn do_cidchar(cmap: &mut CMap, input: &mut Ifreader<'_>, count: i32) -> Result<i32> {
    let mut count = count;
    while count > 0 {
        count -= 1;
        if ifreader_need(input, TOKEN_LEN_MAX * 2)? == 0 {
            return Ok(-1);
        }
        let Some((tok1, tok2)) = get_two_tokens(input)? else {
            return Ok(-1);
        };
        if tok1.is_string() && tok2.is_integer() {
            if let Some(cid) = dst_cid(&tok2)? {
                cmap.CMap_add_cidchar(string_bytes(&tok1)?, cid);
            }
        } else {
            warn!(
                "{}: Invalid CMap mapping record. (ignored)",
                CMAP_PARSE_DEBUG_STR
            );
        }
    }
    check_next_token(input, b"endcidchar")
}

/// `MATCH_NAME`.
fn match_name(t: &PstObj, n: &[u8]) -> Result<bool> {
    Ok(t.is_name() && t.pst_length_of()? as usize == n.len() && t.pst_data_ptr()?.starts_with(n))
}

/// `MATCH_OP`.
fn match_op(t: &PstObj, n: &[u8]) -> Result<bool> {
    Ok(
        t.is_unknown()
            && t.pst_length_of()? as usize == n.len()
            && t.pst_data_ptr()?.starts_with(n),
    )
}

/// `do_cidsysteminfo`.
fn do_cidsysteminfo(cmap: &mut CMap, input: &mut Ifreader<'_>) -> Result<i32> {
    let mut csi = CidSysInfo {
        registry: None,
        ordering: None,
        supplement: -1,
    };
    let mut simpledict = false;
    let mut error = 0;

    ifreader_need(input, TOKEN_LEN_MAX * 2)?;
    // Assuming /CIDSystemInfo 3 dict dup begin .... end def
    // or /CIDSystemInfo << ... >> def
    while let Some(tok1) = get_token(input)? {
        if tok1.is_mark() {
            simpledict = true;
            break;
        } else if match_op(&tok1, b"begin")? {
            simpledict = false;
            break;
        }
        // continue
    }
    while error == 0 {
        let Some(tok1) = get_token(input)? else {
            break;
        };
        if match_op(&tok1, b">>")? && simpledict {
            break;
        } else if match_op(&tok1, b"end")? && !simpledict {
            break;
        } else if match_name(&tok1, b"Registry")? {
            if let Some(tok2) = get_token(input)? {
                if !tok2.is_string() {
                    error = -1;
                } else if !simpledict && check_next_token(input, b"def")? != 0 {
                    error = -1;
                }
                if error == 0 {
                    csi.registry = tok2.pst_getSV()?;
                }
                continue;
            }
        }
        if match_name(&tok1, b"Ordering")? {
            if let Some(tok2) = get_token(input)? {
                if !tok2.is_string() {
                    error = -1;
                } else if !simpledict && check_next_token(input, b"def")? != 0 {
                    error = -1;
                }
                if error == 0 {
                    csi.ordering = tok2.pst_getSV()?;
                }
                continue;
            }
        }
        if match_name(&tok1, b"Supplement")? {
            if let Some(tok2) = get_token(input)? {
                if !tok2.is_integer() {
                    error = -1;
                } else if !simpledict && check_next_token(input, b"def")? != 0 {
                    error = -1;
                }
                if error == 0 {
                    csi.supplement = tok2.pst_getIV()?;
                }
                continue;
            }
        }
    }
    if error == 0 && check_next_token(input, b"def")? != 0 {
        error = -1;
    }
    if error == 0 && csi.registry.is_some() && csi.ordering.is_some() && csi.supplement >= 0 {
        cmap.CMap_set_CIDSysInfo(Some(&csi));
    }
    Ok(error)
}

/// `CMap_parse_check_sig`: 0 for a CMap resource, else -1 (rewinds).
pub fn CMap_parse_check_sig(fp: &mut MemFile) -> i32 {
    let mut result = -1;
    let mut sig = [0u8; CMAP_SIG_MAX + 1];

    fp.rewind();
    if fp.read_into(&mut sig[..CMAP_SIG_MAX]) != CMAP_SIG_MAX {
        result = -1;
    } else {
        sig[CMAP_SIG_MAX] = 0;
        if &sig[..4] != b"%!PS" {
            result = -1;
        } else {
            let rest = cstr(&sig[4..]);
            let key = b"Resource-CMap";
            if rest.windows(key.len()).any(|w| w == key) {
                result = 0;
            }
        }
    }
    fp.rewind();
    result
}

impl Dpx {
    /// `CMap_parse`: -1 on error, else `CMap_is_valid` (0 or 1). `cmap`
    /// is a local (a `usecmap` loads through the cache).
    pub fn CMap_parse(&mut self, cmap: &mut CMap, fp: &mut MemFile) -> Result<i32> {
        let mut status = 0;
        let mut tmpint = -1;

        // file_size(fp): the size, and a rewind.
        let size = fp.len();
        fp.rewind();
        let mut input = ifreader_create(fp, size, INPUT_BUF_SIZE - 1);

        while status >= 0 {
            ifreader_read(&mut input, INPUT_BUF_SIZE / 2)?;
            let Some(tok1) = get_token(&mut input)? else {
                break;
            };
            if match_name(&tok1, b"CMapName")? {
                match get_token(&mut input)? {
                    Some(tok2)
                        if (tok2.is_name() || tok2.is_string())
                            && check_next_token(&mut input, b"def")? >= 0 =>
                    {
                        cmap.CMap_set_name(tok2.pst_data_ptr()?);
                    }
                    _ => status = -1,
                }
            } else if match_name(&tok1, b"CMapType")? {
                match get_token(&mut input)? {
                    Some(tok2)
                        if tok2.is_integer() && check_next_token(&mut input, b"def")? >= 0 =>
                    {
                        cmap.CMap_set_type(tok2.pst_getIV()?);
                    }
                    _ => status = -1,
                }
            } else if match_name(&tok1, b"WMode")? {
                match get_token(&mut input)? {
                    Some(tok2)
                        if tok2.is_integer() && check_next_token(&mut input, b"def")? >= 0 =>
                    {
                        cmap.CMap_set_wmode(tok2.pst_getIV()?);
                    }
                    _ => status = -1,
                }
            } else if match_name(&tok1, b"CIDSystemInfo")? {
                status = do_cidsysteminfo(cmap, &mut input)?;
            } else if match_name(&tok1, b"Version")?
                || match_name(&tok1, b"UIDOffset")?
                || match_name(&tok1, b"XUID")?
            {
                // Ignore
            } else if tok1.is_name() {
                // Possibly usecmap comes next
                if let Some(tok2) = get_token(&mut input)? {
                    if match_op(&tok2, b"usecmap")? {
                        let id = self.CMap_cache_find(cstr(tok1.pst_data_ptr()?))?;
                        if id < 0 {
                            status = -1;
                        } else {
                            self.CMap_set_usecmap(cmap, id)?;
                        }
                    }
                }
            } else if match_op(&tok1, b"begincodespacerange")? {
                status = do_codespacerange(cmap, &mut input, tmpint)?;
            } else if match_op(&tok1, b"beginnotdefrange")? {
                status = do_notdefrange(cmap, &mut input, tmpint)?;
            } else if match_op(&tok1, b"beginnotdefchar")? {
                status = do_notdefchar(cmap, &mut input, tmpint)?;
            } else if match_op(&tok1, b"beginbfrange")? {
                status = do_bfrange(cmap, &mut input, tmpint)?;
            } else if match_op(&tok1, b"beginbfchar")? {
                status = do_bfchar(cmap, &mut input, tmpint)?;
            } else if match_op(&tok1, b"begincidrange")? {
                status = do_cidrange(cmap, &mut input, tmpint)?;
            } else if match_op(&tok1, b"begincidchar")? {
                status = do_cidchar(cmap, &mut input, tmpint)?;
            } else if tok1.is_integer() {
                tmpint = tok1.pst_getIV()?;
            } // else Simply ignore
        }

        if status < 0 {
            Ok(-1)
        } else {
            Ok(i32::from(self.CMap_is_valid(cmap)?))
        }
    }
}
