use log::{LevelFilter, Log, Metadata, Record};
use std::sync::Mutex;
use std::time::Instant;

struct BufferedLogger {
    start: Instant,
    entries: Mutex<Vec<String>>,
}

impl BufferedLogger {
    fn new() -> Self {
        Self {
            start: Instant::now(),
            entries: Mutex::new(Vec::new()),
        }
    }
}

impl Log for BufferedLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= log::max_level()
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let elapsed = self.start.elapsed();
        let line = format!(
            "[+{:.3}s] [{:<5}] {}",
            elapsed.as_secs_f64(),
            record.level().to_string(),
            record.args()
        );
        self.entries.lock().unwrap().push(line);
    }

    fn flush(&self) {}
}

static LOGGER: std::sync::OnceLock<BufferedLogger> = std::sync::OnceLock::new();

fn logger() -> &'static BufferedLogger {
    LOGGER.get().expect("logging::init must be called first")
}

pub fn init(max_level: LevelFilter) -> Result<(), log::SetLoggerError> {
    let logger = LOGGER.get_or_init(BufferedLogger::new);
    let _ = log::set_logger(logger);
    log::set_max_level(max_level);
    Ok(())
}

#[cfg(test)]
pub fn reset(max_level: LevelFilter) -> Result<(), log::SetLoggerError> {
    init(max_level)?;
    dump();
    Ok(())
}

pub fn dump() {
    let entries = {
        let mut entries = logger().entries.lock().unwrap();
        std::mem::take(&mut *entries)
    };
    for line in entries {
        println!("{line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The logger is global state (entries buffer + max level), so tests that
    /// touch it must not run in parallel.
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn buffered_logger_captures_level_and_message() {
        let _guard = TEST_LOCK.lock().unwrap();
        reset(LevelFilter::Debug).unwrap();
        log::debug!("scroll offset {}", 42);
        let entries = logger().entries.lock().unwrap();
        assert_eq!(entries.len(), 1);
        assert!(entries[0].contains("[DEBUG]"));
        assert!(entries[0].contains("scroll offset 42"));
    }

    #[test]
    fn logger_filters_below_max_level() {
        let _guard = TEST_LOCK.lock().unwrap();
        reset(LevelFilter::Info).unwrap();
        log::debug!("hidden");
        log::info!("shown");
        let entries = logger().entries.lock().unwrap();
        assert_eq!(entries.len(), 1);
        assert!(entries[0].contains("[INFO"));
    }
}
