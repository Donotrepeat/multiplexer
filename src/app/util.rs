use std::sync::{Mutex, MutexGuard};

/// Compute the pane dimensions from the host terminal size, leaving one cell
/// on each side for the block border drawn around each pane.
pub fn initialize_pane_size(row: u16, col: u16) -> (u16, u16) {
    let pane_rows = row.saturating_sub(2);
    let pane_cols = col.saturating_sub(2);
    (pane_rows, pane_cols)
}

/// Lock `mutex`, logging and recovering if the previous holder panicked.
///
/// The UI should not hang because one pane panicked, but recovering silently
/// makes the crash invisible; the log line keeps it observable in the dump.
pub fn lock_or_recover<'a, T>(mutex: &'a Mutex<T>, what: &str) -> MutexGuard<'a, T> {
    mutex.lock().unwrap_or_else(|e| {
        log::error!("recovered poisoned lock ({what}): {e}");
        e.into_inner()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn poisoned_lock_is_recovered() {
        let mutex = Arc::new(Mutex::new(41));
        let poisoner = Arc::clone(&mutex);
        let _ = std::thread::spawn(move || {
            let _guard = poisoner.lock().unwrap();
            panic!("poison the mutex on purpose");
        })
        .join();

        let guard = lock_or_recover(&mutex, "test lock");
        assert_eq!(*guard, 41);
    }
}
