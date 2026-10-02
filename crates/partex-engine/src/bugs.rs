//! The pdfTeX bugs partex reproduces for byte-identical output.
//!
//! Each is a behavior of pdfTeX's code that departs from what the code
//! evidently means (a stale pointer, a wrong side), found while matching
//! pdfTeX. The site that reproduces one carries a `PDFTEX_BUG(<name>)`
//! comment saying what pdfTeX does and why it is wrong; a flag here
//! switches it off, for the output pdfTeX would give without the bug.
//! All are on by default: partex's output is pdfTeX's.

/// Which pdfTeX bugs to reproduce.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PdftexBugs {
    /// `lig_margin_kern_var`: in `try_break`'s variations of the marginal
    /// kerns, `char_pw` of a ligature at the margin is 0 (it gets the
    /// ligature's `lig_char`, which is no char node), while its expanded
    /// copy's is the character's.
    pub lig_margin_kern_var: bool,
}

crate::persist_struct!(PdftexBugs {
    lig_margin_kern_var
});

impl Default for PdftexBugs {
    fn default() -> Self {
        Self::ALL
    }
}

impl PdftexBugs {
    /// Every bug reproduced (pdfTeX's output).
    pub const ALL: Self = Self {
        lig_margin_kern_var: true,
    };
    /// None reproduced.
    pub const NONE: Self = Self {
        lig_margin_kern_var: false,
    };

    /// The bugs by name, with a one-line description each.
    pub const NAMES: &[(&str, &str)] = &[(
        "lig_margin_kern_var",
        "a ligature at the margin skews the marginal kerns' variation (try_break)",
    )];

    /// The flag named `name`, if there is one.
    pub fn flag(&mut self, name: &str) -> Option<&mut bool> {
        match name {
            "lig_margin_kern_var" => Some(&mut self.lig_margin_kern_var),
            _ => None,
        }
    }

    /// Parses a switch list: comma-separated `all`, `none`, `name` (on)
    /// and `-name` (off), applied left to right from all on. Returns the
    /// unknown names as the error.
    ///
    /// # Errors
    /// An unknown bug name.
    pub fn parse(spec: &str) -> Result<Self, &str> {
        let mut b = Self::ALL;
        for item in spec.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            match item {
                "all" => b = Self::ALL,
                "none" => b = Self::NONE,
                _ => {
                    let (name, on) = match item.strip_prefix('-') {
                        Some(n) => (n, false),
                        None => (item, true),
                    };
                    *b.flag(name).ok_or(item)? = on;
                }
            }
        }
        Ok(b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_switch_lists() {
        assert_eq!(PdftexBugs::parse(""), Ok(PdftexBugs::ALL));
        assert_eq!(PdftexBugs::parse("none"), Ok(PdftexBugs::NONE));
        assert_eq!(
            PdftexBugs::parse("-lig_margin_kern_var"),
            Ok(PdftexBugs::NONE)
        );
        assert_eq!(
            PdftexBugs::parse("none,lig_margin_kern_var"),
            Ok(PdftexBugs::ALL)
        );
        assert_eq!(PdftexBugs::parse("nope"), Err("nope"));
        for (name, _) in PdftexBugs::NAMES {
            assert!(PdftexBugs::ALL.clone().flag(name).is_some());
        }
    }
}
