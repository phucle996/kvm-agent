use std::error::Error;
use std::io;
use std::panic;

use tracing::{Level, Span};
use tracing_appender::non_blocking;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::filter::{filter_fn, LevelFilter};
use tracing_subscriber::fmt;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::Layer;

use crate::config::LogConfig;

pub struct LoggingHandle {
    _stdout_guard: WorkerGuard,
    _stderr_guard: WorkerGuard,
}

pub fn init(cfg: &LogConfig) -> Result<LoggingHandle, Box<dyn Error + Send + Sync>> {
    let (stdout_writer, stdout_guard) = non_blocking(io::stdout());
    let (stderr_writer, stderr_guard) = non_blocking(io::stderr());

    let min_level = parse_level_filter(&cfg.level).unwrap_or(LevelFilter::INFO);
    let stdout_filter = filter_fn(move |metadata| {
        level_allowed(metadata.level(), min_level) && *metadata.level() <= Level::WARN
    });
    let stderr_filter = filter_fn(move |metadata| {
        level_allowed(metadata.level(), min_level) && *metadata.level() >= Level::ERROR
    });

    let json = cfg.format.trim().eq_ignore_ascii_case("json");
    if json {
        let stdout_layer = fmt::layer()
            .json()
            .with_ansi(false)
            .with_writer(stdout_writer)
            .with_target(false)
            .flatten_event(true)
            .with_current_span(true)
            .with_span_list(false)
            .with_filter(stdout_filter);

        let stderr_layer = fmt::layer()
            .json()
            .with_ansi(false)
            .with_writer(stderr_writer)
            .with_target(false)
            .flatten_event(true)
            .with_current_span(true)
            .with_span_list(false)
            .with_filter(stderr_filter);

        tracing_subscriber::registry()
            .with(stdout_layer)
            .with(stderr_layer)
            .try_init()?;
    } else {
        let stdout_layer = fmt::layer()
            .with_ansi(true)
            .with_writer(stdout_writer)
            .with_target(false)
            .with_filter(stdout_filter);

        let stderr_layer = fmt::layer()
            .with_ansi(true)
            .with_writer(stderr_writer)
            .with_target(false)
            .with_filter(stderr_filter);

        tracing_subscriber::registry()
            .with(stdout_layer)
            .with(stderr_layer)
            .try_init()?;
    }

    let service = cfg.service.clone();
    let environment = cfg.environment.clone();
    let host_id = cfg.host_id.clone();
    panic::set_hook(Box::new(move |panic_info| {
        let location = panic_info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_else(|| "unknown".to_string());
        let payload = match panic_info.payload().downcast_ref::<&str>() {
            Some(v) => (*v).to_string(),
            None => match panic_info.payload().downcast_ref::<String>() {
                Some(v) => v.clone(),
                None => "panic payload unavailable".to_string(),
            },
        };

        tracing::error!(
            service = %service,
            environment = %environment,
            host_id = %host_id,
            component = "runtime",
            operation = "panic",
            status = "fatal",
            error_code = "PANIC",
            error_message = %payload,
            location = %location,
            "fatal panic captured"
        );
    }));

    Ok(LoggingHandle {
        _stdout_guard: stdout_guard,
        _stderr_guard: stderr_guard,
    })
}

pub fn app_span(cfg: &LogConfig) -> Span {
    tracing::info_span!(
        "vm_agent",
        service = %cfg.service,
        environment = %cfg.environment,
        host_id = %cfg.host_id
    )
}

pub fn redact(input: &str) -> String {
    if input.is_empty() {
        return String::new();
    }
    if input.len() <= 6 {
        return "***".to_string();
    }
    let left = &input[..3];
    let right = &input[input.len() - 3..];
    format!("{left}***{right}")
}

pub fn fatal_exit(component: &str, operation: &str, error_code: &str, error_message: &str) -> ! {
    tracing::error!(
        component = %component,
        operation = %operation,
        status = "fatal",
        error_code = %error_code,
        error_message = %error_message,
        "unrecoverable error, terminating process"
    );
    std::process::exit(1);
}

fn parse_level_filter(raw: &str) -> Option<LevelFilter> {
    match raw.trim().to_ascii_uppercase().as_str() {
        "TRACE" => Some(LevelFilter::TRACE),
        "DEBUG" => Some(LevelFilter::DEBUG),
        "INFO" => Some(LevelFilter::INFO),
        "WARN" | "WARNING" => Some(LevelFilter::WARN),
        "ERROR" => Some(LevelFilter::ERROR),
        "OFF" => Some(LevelFilter::OFF),
        _ => None,
    }
}

fn level_allowed(level: &Level, min: LevelFilter) -> bool {
    match min {
        LevelFilter::OFF => false,
        LevelFilter::ERROR => *level == Level::ERROR,
        LevelFilter::WARN => matches!(*level, Level::WARN | Level::ERROR),
        LevelFilter::INFO => matches!(*level, Level::INFO | Level::WARN | Level::ERROR),
        LevelFilter::DEBUG => {
            matches!(
                *level,
                Level::DEBUG | Level::INFO | Level::WARN | Level::ERROR
            )
        }
        LevelFilter::TRACE => true,
    }
}
