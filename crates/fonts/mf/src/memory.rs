//! Parts 10 and 11 of `mf.web`: dynamic memory allocation, memory layout.

use crate::{Mf, Pointer, R, consts::*};

impl Mf {
    // §175
    #[inline]
    pub(crate) fn sentinel(&self) -> Pointer {
        self.mem_top
    }
    #[inline]
    pub(crate) fn temp_head(&self) -> Pointer {
        self.mem_top - 1
    }
    #[inline]
    pub(crate) fn hold_head(&self) -> Pointer {
        self.mem_top - 2
    }
    #[inline]
    pub(crate) fn hi_mem_stat_min(&self) -> Pointer {
        self.mem_top - 2
    }

    // §163
    pub(crate) fn get_avail(&mut self) -> R<Pointer> {
        let mut p = self.avail;
        if p != NULL {
            self.avail = self.link(self.avail);
        } else if self.mem_end < self.mem_max {
            self.mem_end += 1;
            p = self.mem_end;
        } else {
            self.hi_mem_min -= 1;
            p = self.hi_mem_min;
            if self.hi_mem_min <= self.lo_mem_max {
                self.runaway()?;
                return self.overflow("main memory size", self.mem_max + 1 - MEM_MIN);
            }
        }
        self.set_link(p, NULL);
        self.dyn_used += 1;
        Ok(p)
    }
    // §164
    #[inline]
    pub(crate) fn free_avail(&mut self, p: Pointer) {
        self.set_link(p, self.avail);
        self.avail = p;
        self.dyn_used -= 1;
    }
    // §165
    #[inline]
    pub(crate) fn fast_get_avail(&mut self) -> R<Pointer> {
        let p = self.avail;
        if p == NULL {
            self.get_avail()
        } else {
            self.avail = self.link(p);
            self.set_link(p, NULL);
            self.dyn_used += 1;
            Ok(p)
        }
    }
    #[inline]
    pub(crate) fn is_empty(&self, p: Pointer) -> bool {
        self.link(p) == EMPTY_FLAG
    }

    // §167
    pub(crate) fn get_node(&mut self, s: i32) -> R<Pointer> {
        'restart: loop {
            let mut p = self.rover;
            loop {
                // §169
                let mut q = p + self.node_size(p);
                while self.is_empty(q) {
                    let t = self.rlink(q);
                    let tt = self.llink(q);
                    if q == self.rover {
                        self.rover = t;
                    }
                    self.set_llink(t, tt);
                    self.set_rlink(tt, t);
                    q += self.node_size(q);
                }
                let r = q - s;
                if r > p + 1 {
                    // §170
                    self.set_node_size(p, r - p);
                    self.rover = p;
                    return Ok(self.found_node(r, s));
                }
                if r == p && self.rlink(p) != p {
                    // §171
                    self.rover = self.rlink(p);
                    let t = self.llink(p);
                    self.set_llink(self.rover, t);
                    self.set_rlink(t, self.rover);
                    return Ok(self.found_node(r, s));
                }
                self.set_node_size(p, q - p);
                p = self.rlink(p);
                if p == self.rover {
                    break;
                }
            }
            if s == 0o10000000000 {
                return Ok(MAX_HALFWORD);
            }
            if self.lo_mem_max + 2 < self.hi_mem_min
                && self.lo_mem_max + 2 <= MEM_MIN + MAX_HALFWORD
            {
                // §168
                let mut t = if self.hi_mem_min - self.lo_mem_max >= 1998 {
                    self.lo_mem_max + 1000
                } else {
                    self.lo_mem_max + 1 + (self.hi_mem_min - self.lo_mem_max) / 2
                };
                if t > MEM_MIN + MAX_HALFWORD {
                    t = MEM_MIN + MAX_HALFWORD;
                }
                let p = self.llink(self.rover);
                let q = self.lo_mem_max;
                self.set_rlink(p, q);
                self.set_llink(self.rover, q);
                self.set_rlink(q, self.rover);
                self.set_llink(q, p);
                self.set_link(q, EMPTY_FLAG);
                self.set_node_size(q, t - self.lo_mem_max);
                self.lo_mem_max = t;
                self.set_link(self.lo_mem_max, NULL);
                self.set_info(self.lo_mem_max, NULL);
                self.rover = q;
                continue 'restart;
            }
            return self.overflow("main memory size", self.mem_max + 1 - MEM_MIN);
        }
    }
    pub(crate) fn found_node(&mut self, r: Pointer, s: i32) -> Pointer {
        self.set_link(r, NULL);
        self.var_used += s;
        r
    }

    // §172
    pub(crate) fn free_node(&mut self, p: Pointer, s: i32) {
        self.set_node_size(p, s);
        self.set_link(p, EMPTY_FLAG);
        let q = self.llink(self.rover);
        self.set_llink(p, q);
        self.set_rlink(p, self.rover);
        self.set_llink(self.rover, p);
        self.set_rlink(q, p);
        self.var_used -= s;
    }

    // §173
    pub(crate) fn sort_avail(&mut self) -> R {
        let _ = self.get_node(0o10000000000)?;
        let mut p = self.rlink(self.rover);
        self.set_rlink(self.rover, MAX_HALFWORD);
        let old_rover = self.rover;
        while p != old_rover {
            // §174
            if p < self.rover {
                let q = p;
                p = self.rlink(q);
                self.set_rlink(q, self.rover);
                self.rover = q;
            } else {
                let mut q = self.rover;
                while self.rlink(q) < p {
                    q = self.rlink(q);
                }
                let r = self.rlink(p);
                self.set_rlink(p, self.rlink(q));
                self.set_rlink(q, p);
                p = r;
            }
        }
        p = self.rover;
        while self.rlink(p) != MAX_HALFWORD {
            let r = self.rlink(p);
            self.set_llink(r, p);
            p = r;
        }
        self.set_rlink(p, self.rover);
        self.set_llink(self.rover, p);
        Ok(())
    }

    // §177
    pub(crate) fn flush_list(&mut self, p: Pointer) {
        if p >= self.hi_mem_min && p != self.sentinel() {
            let mut r = p;
            let mut q;
            loop {
                q = r;
                r = self.link(r);
                self.dyn_used -= 1;
                if r < self.hi_mem_min {
                    break;
                }
                if r == self.sentinel() {
                    break;
                }
            }
            self.set_link(q, self.avail);
            self.avail = p;
        }
    }
    pub(crate) fn flush_node_list(&mut self, mut p: Pointer) {
        while p != NULL {
            let q = p;
            p = self.link(p);
            if q < self.hi_mem_min {
                self.free_node(q, 2);
            } else {
                self.free_avail(q);
            }
        }
    }
}
