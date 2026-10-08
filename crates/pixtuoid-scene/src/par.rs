//! A frame's independent work split across cores. A frame that follows the
//! TUI's sleep between ticks runs on cooled cores, several times slower than
//! the same frame run back to back (`examples/pacing.rs`'s paced and hot
//! runs), so work a frame can split, it splits.

/// The cores this process may run on; one where the platform can't say,
/// as wasm32 can't, so the work stays on the calling thread there.
pub fn cores() -> usize {
    std::thread::available_parallelism().map_or(1, std::num::NonZero::get)
}

/// `f` of each of `items`, in order, on up to `threads` scoped threads in
/// contiguous shares; on this thread alone when that is one.
pub fn map<T: Sync, U: Send>(items: &[T], threads: usize, f: impl Fn(&T) -> U + Sync) -> Vec<U> {
    let threads = threads.min(items.len()).max(1);
    if threads == 1 {
        return items.iter().map(f).collect();
    }
    let f = &f;
    std::thread::scope(|s| {
        let shares: Vec<_> = items
            .chunks(items.len().div_ceil(threads))
            .map(|share| s.spawn(move || share.iter().map(f).collect::<Vec<_>>()))
            .collect();
        shares
            .into_iter()
            .flat_map(|share| {
                share
                    .join()
                    .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
            })
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Split or not, the results come back in the items' order, one each.
    #[test]
    fn a_split_keeps_the_items_order() {
        let items: Vec<u32> = (0..37).collect();
        for threads in [0, 1, 2, 3, 8, 64] {
            assert_eq!(
                map(&items, threads, |&i| i * 2),
                items.iter().map(|&i| i * 2).collect::<Vec<_>>(),
                "{threads} threads"
            );
        }
        assert!(map(&[] as &[u32], 4, |&i| i).is_empty());
    }

    /// More than one thread runs the work when it is asked to.
    #[test]
    fn a_split_runs_on_more_than_one_thread() {
        let items: Vec<u32> = (0..8).collect();
        let ids: std::collections::HashSet<_> = map(&items, 4, |_| std::thread::current().id())
            .into_iter()
            .collect();
        assert_eq!(ids.len(), 4);
    }
}
