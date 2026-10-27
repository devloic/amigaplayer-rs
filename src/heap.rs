//! First-fit allocator for guest memory (backs exec AllocMem and friends).

use std::collections::BTreeMap;

pub struct Heap {
    free: BTreeMap<u32, u32>, // start -> size
    used: BTreeMap<u32, u32>,
}

impl Heap {
    pub fn new(start: u32, end: u32) -> Self {
        let mut free = BTreeMap::new();
        free.insert(start, end - start);
        Heap { free, used: BTreeMap::new() }
    }

    pub fn alloc(&mut self, size: u32) -> Option<u32> {
        let size = (size.max(1) + 7) & !7;
        let (&start, &len) = self.free.iter().find(|&(_, &len)| len >= size)?;
        self.free.remove(&start);
        if len > size {
            self.free.insert(start + size, len - size);
        }
        self.used.insert(start, size);
        Some(start)
    }

    /// `alloc` from the end of the highest free block that fits.
    pub fn alloc_top(&mut self, size: u32) -> Option<u32> {
        let size = (size.max(1) + 7) & !7;
        let (&start, &len) = self.free.iter().rev().find(|&(_, &len)| len >= size)?;
        if len > size {
            self.free.insert(start, len - size);
        } else {
            self.free.remove(&start);
        }
        let a = start + len - size;
        self.used.insert(a, size);
        Some(a)
    }

    /// Frees a block previously returned by `alloc`. Returns its size.
    pub fn free(&mut self, start: u32) -> Option<u32> {
        let size = self.used.remove(&start)?;
        let mut s = start;
        let mut l = size;
        if let Some((&ps, &pl)) = self.free.range(..s).next_back() {
            if ps + pl == s {
                self.free.remove(&ps);
                s = ps;
                l += pl;
            }
        }
        if let Some(&nl) = self.free.get(&(s + l)) {
            self.free.remove(&(s + l));
            l += nl;
        }
        self.free.insert(s, l);
        Some(size)
    }

    /// Frees whatever allocation contains `addr` (FreeMem may be handed a
    /// pointer with a size; only the start matters to us).
    pub fn free_containing(&mut self, addr: u32) -> Option<u32> {
        let (&s, &l) = self.used.range(..=addr).next_back()?;
        if addr < s + l {
            self.free(s)
        } else {
            None
        }
    }

    pub fn available(&self) -> u32 {
        self.free.values().sum()
    }

    pub fn largest(&self) -> u32 {
        self.free.values().copied().max().unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alloc_free_coalesce() {
        let mut h = Heap::new(0x1000, 0x2000);
        let a = h.alloc(0x100).unwrap();
        let b = h.alloc(0x100).unwrap();
        let c = h.alloc(0x100).unwrap();
        assert_eq!((a, b, c), (0x1000, 0x1100, 0x1200));
        h.free(b);
        h.free(a);
        h.free(c);
        assert_eq!(h.largest(), 0x1000);
    }

    #[test]
    fn alloc_top_leaves_the_bottom_alone() {
        let mut h = Heap::new(0x1000, 0x2000);
        let t = h.alloc_top(0x100).unwrap();
        assert_eq!(t, 0x1F00);
        assert_eq!(h.alloc(0x100), Some(0x1000));
        h.free(t);
        assert_eq!(h.largest(), 0x1000 - 0x100);
        assert_eq!(h.alloc_top(0xF00), Some(0x1100));
        assert_eq!(h.alloc_top(8), None);
    }
}
