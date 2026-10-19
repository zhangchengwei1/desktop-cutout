//! Bounded pixel work, independent of the task scheduler and ORT's own pool.
use rayon::{prelude::*, ThreadPool, ThreadPoolBuilder};
use std::sync::OnceLock;

const MIN_ELEMENTS: usize = 256 * 1024;

#[cfg(test)]
thread_local! { static SERIAL: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }

#[cfg(test)]
pub(crate) fn serial<T>(work: impl FnOnce() -> T) -> T {
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            SERIAL.set(self.0);
        }
    }
    let _restore = Restore(SERIAL.replace(true));
    work()
}

fn pool() -> Option<&'static ThreadPool> {
    static POOL: OnceLock<Option<ThreadPool>> = OnceLock::new();
    POOL.get_or_init(|| {
        let threads = std::thread::available_parallelism()
            .map_or(1, usize::from)
            .min(4);
        if threads < 2 {
            return None;
        }
        ThreadPoolBuilder::new()
            .num_threads(threads)
            .thread_name(|index| format!("cutout-pixels-{index}"))
            .build()
            .ok()
    })
    .as_ref()
}

/// The callback receives the chunk index, including the final partial chunk.
/// Small buffers and machines unable to create workers use the same serial path.
pub(crate) fn chunks_mut<T: Send, F: Fn(usize, &mut [T]) + Send + Sync>(
    values: &mut [T],
    chunk_size: usize,
    work: F,
) {
    #[cfg(test)]
    let allow_parallel = !SERIAL.get();
    #[cfg(not(test))]
    let allow_parallel = true;
    if allow_parallel && values.len() >= MIN_ELEMENTS {
        if let Some(pool) = pool() {
            pool.install(|| {
                values
                    .par_chunks_mut(chunk_size)
                    .enumerate()
                    .for_each(|(i, chunk)| work(i, chunk))
            });
            return;
        }
    }
    values
        .chunks_mut(chunk_size)
        .enumerate()
        .for_each(|(i, chunk)| work(i, chunk));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_keep_indices_and_cover_partial_tail() {
        for length in [19, MIN_ELEMENTS + 19] {
            let mut values = vec![usize::MAX; length];
            chunks_mut(&mut values, 127, |chunk, values| {
                for (offset, value) in values.iter_mut().enumerate() {
                    *value = chunk * 127 + offset;
                }
            });
            assert!(values
                .iter()
                .enumerate()
                .all(|(index, value)| index == *value));
        }
    }
}
