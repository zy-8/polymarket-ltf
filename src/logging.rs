use std::{
    fs, panic,
    path::PathBuf,
    sync::{Once, OnceLock},
};

use tracing_appender::non_blocking::{NonBlocking, WorkerGuard};
use tracing_subscriber::{EnvFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt};

static INIT: Once = Once::new();
static LOG_GUARD: OnceLock<WorkerGuard> = OnceLock::new();

const LOG_DIR_ENV: &str = "POLYMARKET_LTF_LOG_DIR";
const LOG_FILE_SUFFIX: &str = "log";
const LOG_MAX_FILES: usize = 14;
const LOGGING_THREAD_NAME: &str = "polymarket-ltf-logging";

pub fn init() {
    INIT.call_once(|| {
        let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
        let stdout_layer = fmt::layer()
            .compact()
            .with_target(false)
            .with_writer(std::io::stdout);

        match init_file_writer() {
            Ok(non_blocking) => tracing_subscriber::registry()
                .with(filter)
                .with(stdout_layer)
                .with(
                    fmt::layer()
                        .compact()
                        .with_ansi(false)
                        .with_target(true)
                        .with_thread_ids(true)
                        .with_thread_names(true)
                        .with_writer(non_blocking),
                )
                .init(),
            Err(err) => {
                eprintln!("failed to initialize daily log file writer: {err}");
                tracing_subscriber::registry()
                    .with(filter)
                    .with(stdout_layer)
                    .init();
            }
        }

        install_panic_hook();
    });
}

fn init_file_writer() -> std::io::Result<NonBlocking> {
    let log_dir = log_directory();
    fs::create_dir_all(&log_dir)?;

    let file_appender = tracing_appender::rolling::RollingFileAppender::builder()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_suffix(LOG_FILE_SUFFIX)
        .max_log_files(LOG_MAX_FILES)
        .build(log_dir)
        .map_err(std::io::Error::other)?;
    let (non_blocking, guard) = tracing_appender::non_blocking::NonBlockingBuilder::default()
        .lossy(false)
        .thread_name(LOGGING_THREAD_NAME)
        .finish(file_appender);

    let _ = LOG_GUARD.set(guard);

    Ok(non_blocking)
}

fn log_directory() -> PathBuf {
    match std::env::var(LOG_DIR_ENV) {
        Ok(dir) if !dir.trim().is_empty() => PathBuf::from(dir),
        _ => PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("logs"),
    }
}

fn install_panic_hook() {
    panic::set_hook(Box::new(|panic_info| {
        let thread = std::thread::current();
        let thread_name = thread.name().unwrap_or("unnamed");
        let location = panic_info
            .location()
            .map(|location| {
                format!(
                    "{}:{}:{}",
                    location.file(),
                    location.line(),
                    location.column()
                )
            })
            .unwrap_or_else(|| "unknown".to_string());
        let payload = panic_payload(panic_info);

        tracing::error!(
            thread = %thread_name,
            location = %location,
            payload = %payload,
            "application panic"
        );
    }));
}

fn panic_payload(panic_info: &panic::PanicHookInfo<'_>) -> String {
    if let Some(payload) = panic_info.payload().downcast_ref::<&str>() {
        (*payload).to_string()
    } else if let Some(payload) = panic_info.payload().downcast_ref::<String>() {
        payload.clone()
    } else {
        "non-string panic payload".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::Write,
        time::{SystemTime, UNIX_EPOCH},
    };

    #[test]
    fn tracing_appender_daily_writer_creates_normalized_log_file() {
        let unique_suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos();
        let log_dir = std::env::temp_dir().join(format!("polymarket-ltf-logging-{unique_suffix}"));

        fs::create_dir_all(&log_dir).expect("temp log dir should be created");

        let mut appender = tracing_appender::rolling::RollingFileAppender::builder()
            .rotation(tracing_appender::rolling::Rotation::DAILY)
            .filename_suffix(LOG_FILE_SUFFIX)
            .build(&log_dir)
            .expect("rolling file appender should build");
        writeln!(appender, "hello tracing appender").expect("appender should accept log lines");
        appender.flush().expect("appender should flush");

        let entries = fs::read_dir(&log_dir)
            .expect("temp log dir should be readable")
            .filter_map(Result::ok)
            .collect::<Vec<_>>();

        assert_eq!(entries.len(), 1, "expected one daily log file");

        let log_path = entries[0].path();
        let file_name = log_path
            .file_name()
            .and_then(|value| value.to_str())
            .expect("log file name should be valid unicode");
        let contents = fs::read_to_string(&log_path).expect("log file should exist");

        assert_eq!(file_name.len(), "2026-03-20.log".len());
        assert!(
            file_name
                .chars()
                .take(4)
                .all(|value| value.is_ascii_digit())
        );
        assert!(file_name.ends_with(".log"));
        assert!(contents.contains("hello tracing appender"));

        let _ = fs::remove_dir_all(log_dir);
    }
}
