// SPDX-License-Identifier: Apache-2.0
//! One validated value stream may feed sparse postings and another index sink.
#[cfg(test)]
mod tests;

use super::*;
use crate::core::grams::value_gram_iter;

impl Builder {
    pub fn add_value(&mut self, raw: Option<&[u8]>, guard: Option<&str>) -> Result<(), Error> {
        self.feed(raw, guard, |_| {}, false)
    }

    /// Observe every anchored gram, including duplicates, for every configured
    /// view of a non-null value within the byte bound. Observation continues
    /// after sparse saturation, missing guards or whole-file fallback. Null and
    /// overlong values produce no observations; callers must retain their own
    /// fallback flags. The observer cannot supply or alter the indexed grams.
    pub fn add_value_visiting_grams(
        &mut self,
        raw: Option<&[u8]>,
        guard: Option<&str>,
        visit: impl FnMut(u64),
    ) -> Result<(), Error> {
        self.feed(raw, guard, visit, true)
    }

    fn feed(
        &mut self,
        raw: Option<&[u8]>,
        guard: Option<&str>,
        mut visit: impl FnMut(u64),
        observe: bool,
    ) -> Result<(), Error> {
        let protection = WidenOnUnwind(
            self.current
                .as_mut()
                .ok_or(Error::Invalid("no current file"))?,
        );
        let p = &mut *protection.0;
        p.rows = p.rows.checked_add(1).ok_or(Error::Invalid("row count"))?;
        let Some(raw) = raw else { return Ok(()) };
        if p.unrestricted && !observe {
            return Ok(());
        }
        let guard = guard.filter(|g| g.len() <= MAX_ID_BYTES);
        if guard.is_none() || raw.len() > self.index.max_value_bytes {
            p.widen();
        }
        if raw.len() > self.index.max_value_bytes || p.unrestricted && !observe {
            return Ok(());
        }
        // Allocate only if this row adds a posting; saturated grams need none.
        let mut shared_guard = None;
        for &view in &self.index.views {
            let value = view.apply(raw);
            for gram in value_gram_iter(view, &value) {
                visit(gram);
                if let Some(guard) = guard.filter(|_| !p.unrestricted) {
                    p.add_gram_shared(gram, guard, &self.retained, &mut shared_guard);
                }
                if p.unrestricted && !observe {
                    return Ok(());
                }
            }
        }
        Ok(())
    }
}

impl<S: std::hash::BuildHasher> Pending<S> {
    #[cfg(test)]
    fn add_gram(&mut self, gram: u64, guard: &str, retained: &Usage) {
        self.add_gram_shared(gram, guard, retained, &mut None);
    }

    fn add_gram_shared(
        &mut self,
        gram: u64,
        guard: &str,
        retained: &Usage,
        shared: &mut Option<Arc<str>>,
    ) {
        use std::collections::hash_map::Entry;
        // Entry can reserve table storage for a vacant key. At the gram cap,
        // only look up existing slots so fallback never grows the table first.
        let slot = if self.usage.grams + retained.grams >= MAX_GRAMS {
            match self.grams.get_mut(&gram) {
                Some(slot) => slot,
                None => {
                    self.widen();
                    return;
                }
            }
        } else {
            match self.grams.entry(gram) {
                Entry::Occupied(entry) => entry.into_mut(),
                Entry::Vacant(entry) => {
                    self.usage.grams += 1;
                    entry.insert(Some(Box::default()))
                }
            }
        };
        let Some(ids) = slot.as_mut() else { return };
        if ids.contains(guard) {
            return;
        }
        if ids.len() == IDS_PER_GRAM {
            self.usage.postings -= ids.len();
            self.usage.bytes -= ids.iter().map(|id| id.len()).sum::<usize>();
            *slot = None;
            return;
        }
        if self.usage.postings + retained.postings >= MAX_POSTINGS
            || guard.len() > MAX_GUARD_BYTES - self.usage.bytes - retained.bytes
        {
            self.widen();
            return;
        }
        ids.insert(shared.get_or_insert_with(|| Arc::from(guard)).clone());
        self.usage.postings += 1;
        self.usage.bytes += guard.len();
    }
}

// A caller may catch an observer panic and continue using the builder. Never
// let that expose an index with a partially ingested row.
struct WidenOnUnwind<'a>(&'a mut Pending);
impl Drop for WidenOnUnwind<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            self.0.widen();
        }
    }
}
