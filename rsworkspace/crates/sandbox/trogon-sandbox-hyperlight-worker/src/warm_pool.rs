/// Runtimes that are built and have no module loaded, ready for the next lease.
pub(crate) struct WarmPool<W> {
    items: Vec<W>,
    capacity: usize,
}

impl<W> WarmPool<W> {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            items: Vec::with_capacity(capacity),
            capacity,
        }
    }

    pub(crate) fn take(&mut self) -> Option<W> {
        self.items.pop()
    }

    /// Keeps `item` unless the pool is full, in which case it is dropped.
    pub(crate) fn give(&mut self, item: W) {
        if self.items.len() < self.capacity {
            self.items.push(item);
        }
    }
}

#[cfg(test)]
mod tests;
