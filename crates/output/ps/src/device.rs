//! The page device: `setpagedevice` (PageSize), `showpage`, the EPS
//! handling of gs_epsf.ps (`-dEPSCrop`: the page cropped to the
//! %%BoundingBox, or the %%HiResBoundingBox, as their comments are read).

use alloc::vec::Vec;

use crate::interp::{E, Ps, R};
use crate::obj::{Obj, V};

/// gs_epsf.ps's ProcessEPSComment for one DSC line.
pub fn dsc_comment(ps: &mut Ps<'_>, line: &[u8]) {
    // (pdfwrite's DSC processing, gdevpdfp.c: %%Title, %%Creator and
    // %%For into the Info dictionary; %%Orientation other than portrait
    // turns the page, not done here)
    for (c, k) in [
        (&b"%%Title:"[..], "Title"),
        (b"%%Creator:", "Creator"),
        (b"%%For:", "Author"),
    ] {
        if let Some(r) = line.strip_prefix(c) {
            let v: Vec<u8> = r
                .iter()
                .copied()
                .skip_while(|&b| b == b' ' || b == b'\t')
                .collect();
            match ps.dev.dsc_info.iter_mut().find(|(n, _)| *n == k) {
                Some(e) => e.1 = v,
                None => ps.dev.dsc_info.push((k, v)),
            }
            return;
        }
    }
    if let Some(r) = line.strip_prefix(b"%%Orientation:") {
        let v: Vec<u8> = r
            .iter()
            .copied()
            .skip_while(|&b| b == b' ' || b == b'\t')
            .collect();
        if !v.starts_with(b"Portrait") && !v.starts_with(b"portrait") && ps.unsupported.is_none() {
            ps.unsupported = Some(alloc::format!(
                "%%Orientation: {}",
                alloc::string::String::from_utf8_lossy(&v)
            ));
        }
    }
    if ps.eps_state >= 3 {
        if is_end_comments(line) {
            ps.eps_state = 5;
        }
        return;
    }
    let (state, rest) = if let Some(r) = line.strip_prefix(b"%%BoundingBox:") {
        (1, r)
    } else if let Some(r) = line.strip_prefix(b"%%HiResBoundingBox:") {
        (3, r)
    } else {
        if is_end_comments(line) {
            ps.eps_state = 5;
        }
        return;
    };
    if ps.eps_state >= state {
        return;
    }
    // (EPSBoundingBoxParse: four tokens, numbers)
    let mut v = Vec::new();
    let f = alloc::rc::Rc::new(core::cell::RefCell::new(crate::file::File::new(
        alloc::rc::Rc::new(rest.to_vec()),
    )));
    for _ in 0..4 {
        match crate::scan::token(&mut f.borrow_mut()) {
            Ok(Some(crate::scan::Tok::Int(i))) => v.push(i as f64),
            Ok(Some(crate::scan::Tok::Real(r))) => v.push(f64::from(r)),
            _ => break,
        }
    }
    let more = matches!(crate::scan::token(&mut f.borrow_mut()), Ok(Some(_)));
    if v.len() != 4 || more {
        return;
    }
    if ps.dev.s.eps_crop {
        // (EPSBoundingBoxCrop, in PostScript's arithmetic)
        let (llx, lly, urx, ury) = (v[0] as f32, v[1] as f32, v[2] as f32, v[3] as f32);
        let (w, h) = (urx - llx, ury - lly);
        set_page_size(ps, f64::from(w), f64::from(h));
        ps.gs.ctm = ps.gs.ctm.translate(f64::from(-llx), f64::from(-lly));
    }
    ps.eps_state = state;
}

fn is_end_comments(line: &[u8]) -> bool {
    line.starts_with(b"%%EndComments")
        || line.starts_with(b"%%BeginProlog")
        || line.starts_with(b"%%BeginSetup")
}

/// A new page size: the device reopened, the graphics initialised.
pub fn set_page_size(ps: &mut Ps<'_>, w: f64, h: f64) {
    ps.dev.set_page_size(w, h);
    ps.initgraphics();
}

fn op_setpagedevice(ps: &mut Ps<'_>) -> R {
    let d = ps.pop_dict()?;
    if let Some(o) = ps.dget_s(&d, "PageSize") {
        let v = ps.nums_of(&o)?;
        if v.len() >= 2 {
            let (w, h) = (f64::from(v[0] as f32), f64::from(v[1] as f32));
            set_page_size(ps, w, h);
            return Ok(());
        }
    }
    ps.initgraphics();
    Ok(())
}
fn op_currentpagedevice(ps: &mut Ps<'_>) -> R {
    let d = ps.new_dict(20);
    let (w, h) = ps.dev.media;
    let a = ps.arr_obj(vec_of(&[w, h]));
    ps.dput_s(&d, "PageSize", a);
    let r = ps.arr_obj(vec_of(&[720.0, 720.0]));
    ps.dput_s(&d, "HWResolution", r);
    ps.dput_s(&d, "Orientation", Obj::int(0));
    ps.dput_s(&d, "MaxSeparations", Obj::int(4));
    ps.dput_s(&d, "NumCopies", Obj::NULL);
    let pcm = ps.str_obj(b"DeviceCMYK");
    let _ = pcm;
    let n = ps.nobj("pdfwrite");
    ps.dput_s(&d, "OutputDevice", n);
    let b = ps.arr_obj(vec_of(&[0.0, 0.0]));
    ps.dput_s(&d, "Margins", b);
    ps.push(Obj::dict(d));
    Ok(())
}
fn vec_of(v: &[f64]) -> Vec<Obj> {
    v.iter().map(|&x| Obj::real(x as f32)).collect()
}
fn op_showpage(ps: &mut Ps<'_>) -> R {
    ps.showpages += 1;
    if ps.showpages == 1 {
        ps.dev.page_done = true;
    }
    ps.initgraphics();
    Ok(())
}
fn op_copypage(_: &mut Ps<'_>) -> R {
    Ok(())
}
fn op_erasepage(_: &mut Ps<'_>) -> R {
    Ok(())
}
fn op_nulldevice(ps: &mut Ps<'_>) -> R {
    ps.null_device = true;
    Ok(())
}
fn op_setdevice(ps: &mut Ps<'_>) -> R {
    ps.pop()?;
    Ok(())
}
fn op_currentdevice(ps: &mut Ps<'_>) -> R {
    let d = ps.new_dict(2);
    ps.push(Obj::dict(d));
    Ok(())
}
fn op_getdeviceprops(ps: &mut Ps<'_>) -> R {
    ps.push(Obj::mark());
    Ok(())
}
fn op_putdeviceprops(ps: &mut Ps<'_>) -> R {
    let p =
        ps.o.iter()
            .rposition(|o| matches!(o.v, V::Mark))
            .ok_or(crate::interp::Sig::Err(E::UnmatchedMark))?;
    ps.o.truncate(p);
    Ok(())
}

pub fn register(ps: &mut Ps<'_>) {
    let t: &[(&'static str, crate::interp::OpFn)] = &[
        ("setpagedevice", op_setpagedevice),
        ("currentpagedevice", op_currentpagedevice),
        ("showpage", op_showpage),
        ("copypage", op_copypage),
        ("erasepage", op_erasepage),
        ("nulldevice", op_nulldevice),
        ("setdevice", op_setdevice),
        ("currentdevice", op_currentdevice),
        ("getdeviceprops", op_getdeviceprops),
        ("putdeviceprops", op_putdeviceprops),
    ];
    for &(n, f) in t {
        ps.op(n, f);
    }
}
