use std::cell::Cell;
#[rustfmt::skip]
#[derive(Default, Debug)]
pub(crate) struct Statistics {
    cache_hit    : Cell<usize>,
    cache_miss   : Cell<usize>,
    disk_write   : Cell<usize>,
    evictions    : Cell<usize>,
}

impl Statistics {
    pub fn inc_cache_hit(&self) {
        self.cache_hit.set(self.cache_hit.get() + 1);
    }

    pub fn inc_cache_miss(&self) {
        self.cache_miss.set(self.cache_miss.get() + 1);
    }

    pub fn inc_disk_write(&self) {
        self.disk_write.set(self.disk_write.get() + 1);
    }

    pub fn inc_evictions(&self) {
        self.evictions.set(self.evictions.get() + 1);
    }
    #[allow(dead_code)]
    pub fn cache_hit(&self) -> usize {
        self.cache_hit.get()
    }

    #[allow(dead_code)]
    pub fn cache_miss(&self) -> usize {
        self.cache_miss.get()
    }

    #[allow(dead_code)]
    pub fn disk_write(&self) -> usize {
        self.disk_write.get()
    }

    #[allow(dead_code)]
    pub fn evictions(&self) -> usize {
        self.evictions.get()
    }
}
