use std::cell::Cell;
/// Counters for what the cache and the pager have been doing.
#[rustfmt::skip]
#[derive(Default, Debug)]
pub(crate) struct Statistics {
    cache_hit    : Cell<usize>,
    cache_miss   : Cell<usize>,
    disk_write   : Cell<usize>,
    evictions    : Cell<usize>,
}

impl Statistics {
    /// Count a page that was already in the cache.
    pub fn inc_cache_hit(&self) {
        self.cache_hit.set(self.cache_hit.get() + 1);
    }

    /// Count a page that had to be read from disk.
    pub fn inc_cache_miss(&self) {
        self.cache_miss.set(self.cache_miss.get() + 1);
    }

    /// Count a page written back to disk.
    pub fn inc_disk_write(&self) {
        self.disk_write.set(self.disk_write.get() + 1);
    }

    /// Count a page handed back to make room for another.
    pub fn inc_evictions(&self) {
        self.evictions.set(self.evictions.get() + 1);
    }
    /// How many lookups were served from the cache.
    #[allow(dead_code)]
    pub fn cache_hit(&self) -> usize {
        self.cache_hit.get()
    }

    /// How many lookups had to read from disk.
    #[allow(dead_code)]
    pub fn cache_miss(&self) -> usize {
        self.cache_miss.get()
    }

    /// How many pages have been written to disk.
    #[allow(dead_code)]
    pub fn disk_write(&self) -> usize {
        self.disk_write.get()
    }

    /// How many pages have been evicted.
    #[allow(dead_code)]
    pub fn evictions(&self) -> usize {
        self.evictions.get()
    }
}
