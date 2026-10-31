//! Colors in spaces other than the device ones (gdevpdfc.c): the space
//! as a resource (`/R<n> cs`), the color with `scn`.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::color::{Color, Space};
use crate::num::pg;
use crate::pdf::{Device, psdf_round};

/// Writes `c` (a non-device space) for filling or stroking.
pub fn write_color(d: &mut Device, c: &Color, stroke: bool) {
    let vals: Vec<String> = c
        .vals
        .iter()
        .map(|&v| pg(psdf_round(f64::from(v))))
        .collect();
    match &c.cs.space {
        Space::Pattern { .. } => {
            crate::pattern::write_pattern_color(d, c, stroke);
        }
        Space::Cie { n, .. } => {
            let op = match (n, stroke) {
                (1, false) => "g",
                (1, true) => "G",
                (4, false) => "k",
                (4, true) => "K",
                (_, false) => "rg",
                (_, true) => "RG",
            };
            d.put(&format!("{} {op}\n", vals.join(" ")));
        }
        _ => {
            let id = crate::pdfcolor::space_resource(d, c);
            d.put(&format!("/R{id} {}\n", if stroke { "CS" } else { "cs" }));
            d.put(&format!(
                "{} {}\n",
                vals.join(" "),
                if stroke { "SCN" } else { "scn" }
            ));
        }
    }
}

/// The color space's resource object (written once per space).
pub fn space_resource(d: &mut Device, c: &Color) -> i64 {
    if let Some(&(_, id)) = d.images.spaces.iter().find(|(sid, _)| *sid == c.cs.id) {
        d.add_resource("ColorSpace", id);
        return id;
    }
    let id = d.alloc();
    let body = crate::image::space_body(d, &c.cs);
    d.write_separate(id, body.into_bytes());
    d.images.spaces.push((c.cs.id, id));
    d.add_resource("ColorSpace", id);
    id
}
