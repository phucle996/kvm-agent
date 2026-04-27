use std::time::{Duration, Instant};

use anyhow::Result;
use tokio::time::{sleep, timeout};

use crate::app::context::AppContext;
use crate::app::state::AppState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutdownResult {
    Clean,
    TimedOut,
}

pub async fn wait_for_shutdown_signal() -> Result<String> {
    tracing::info!(
        component = "lifecycle",
        operation = "wait_for_shutdown_signal",
        status = "started",
        "waiting for shutdown signal"
    );

    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};

        let mut sigterm = signal(SignalKind::terminate())?;
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                tracing::info!(
                    component = "lifecycle",
                    operation = "shutdown_signal",
                    status = "received",
                    signal = "ctrl_c",
                    "received ctrl_c"
                );
                Ok("ctrl_c".to_string())
            }
            _ = sigterm.recv() => {
                tracing::info!(
                    component = "lifecycle",
                    operation = "shutdown_signal",
                    status = "received",
                    signal = "sigterm",
                    "received sigterm"
                );
                Ok("sigterm".to_string())
            }
        }
    }

    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c().await?;
        tracing::info!(
            component = "lifecycle",
            operation = "shutdown_signal",
            status = "received",
            signal = "ctrl_c",
            "received ctrl_c"
        );
        Ok("ctrl_c".to_string())
    }
}

pub async fn shutdown(ctx: &AppContext, shutdown_timeout: Duration) -> ShutdownResult {
    let started_at = Instant::now();

    // 1. set app state to shutting_down
    ctx.state.set(AppState::ShuttingDown);
    tracing::info!(
        component = "lifecycle",
        operation = "shutdown",
        status = "started",
        timeout_ms = shutdown_timeout.as_millis() as u64,
        "shutdown sequence started"
    );

    // 2. stop accepting new incoming requests
    ctx.grpc_server.stop_accepting();

    // 3. stop accepting new jobs
    ctx.worker_pool.stop_accepting();
    ctx.dispatcher.stop_polling();

    // 4. broadcast shutdown signal to long-running tasks
    ctx.shutdown_token.cancel();
    tracing::info!(
        component = "lifecycle",
        operation = "broadcast_shutdown",
        status = "success",
        "shutdown signal broadcast"
    );

    // 5. drain in-flight work within configured timeout
    let remaining_for_drain = shutdown_timeout.saturating_sub(started_at.elapsed());
    let drained = drain_tasks(ctx, remaining_for_drain).await;

    // 6. close transport listeners
    let remaining_for_transport = shutdown_timeout.saturating_sub(started_at.elapsed());
    let transports_closed = close_transports(ctx, remaining_for_transport).await;

    // 7. close external clients and dependencies
    let remaining_for_dependencies = shutdown_timeout.saturating_sub(started_at.elapsed());
    let dependencies_closed = close_dependencies(ctx, remaining_for_dependencies).await;

    // 8. flush final logs if needed (non-blocking logger flushes on guard drop in main)
    let elapsed_ms = started_at.elapsed().as_millis() as u64;

    // 9. return clean shutdown result or forced-timeout result
    let clean = drained
        && transports_closed
        && dependencies_closed
        && elapsed_ms <= shutdown_timeout.as_millis() as u64;
    if clean {
        ctx.state.set(AppState::Stopped);
        tracing::info!(
            component = "lifecycle",
            operation = "shutdown",
            status = "success",
            duration_ms = elapsed_ms,
            "shutdown completed cleanly"
        );
        ShutdownResult::Clean
    } else {
        ctx.state.set(AppState::Stopped);
        tracing::warn!(
            component = "lifecycle",
            operation = "shutdown",
            status = "timeout",
            duration_ms = elapsed_ms,
            "shutdown completed with timeout/forced result"
        );
        ShutdownResult::TimedOut
    }
}

pub async fn drain_tasks(ctx: &AppContext, timeout_duration: Duration) -> bool {
    tracing::info!(
        component = "lifecycle",
        operation = "drain_tasks",
        status = "started",
        timeout_ms = timeout_duration.as_millis() as u64,
        "draining tracked tasks and worker in-flight jobs"
    );

    let started = Instant::now();

    ctx.tasks.close();
    ctx.worker_pool.drain(timeout_duration).await;

    let remaining = timeout_duration.saturating_sub(started.elapsed());
    if remaining.is_zero() {
        tracing::warn!(
            component = "lifecycle",
            operation = "drain_tasks",
            status = "timeout",
            "no time left to wait for tracked tasks"
        );
        return false;
    }

    match timeout(remaining, ctx.tasks.wait()).await {
        Ok(_) => {
            tracing::info!(
                component = "lifecycle",
                operation = "drain_tasks",
                status = "success",
                "all tracked tasks drained"
            );
            true
        }
        Err(_) => {
            tracing::warn!(
                component = "lifecycle",
                operation = "drain_tasks",
                status = "timeout",
                "timed out while waiting for tracked tasks"
            );
            false
        }
    }
}

pub async fn close_transports(ctx: &AppContext, timeout_duration: Duration) -> bool {
    tracing::info!(
        component = "lifecycle",
        operation = "close_transports",
        status = "started",
        timeout_ms = timeout_duration.as_millis() as u64,
        "closing transport listeners"
    );

    if timeout_duration.is_zero() {
        tracing::warn!(
            component = "lifecycle",
            operation = "close_transports",
            status = "timeout",
            "no time left for transport shutdown"
        );
        return false;
    }

    let grpc_ok = ctx.grpc_server.shutdown(timeout_duration).await;

    if grpc_ok {
        tracing::info!(
            component = "lifecycle",
            operation = "close_transports",
            status = "success",
            "grpc transport closed"
        );
        true
    } else {
        tracing::warn!(
            component = "lifecycle",
            operation = "close_transports",
            status = "timeout",
            "grpc transport failed to close before timeout"
        );
        false
    }
}

pub async fn close_dependencies(_ctx: &AppContext, timeout_duration: Duration) -> bool {
    tracing::info!(
        component = "lifecycle",
        operation = "close_dependencies",
        status = "started",
        timeout_ms = timeout_duration.as_millis() as u64,
        "closing external dependencies"
    );

    let close_future = async {
        // Placeholder dependency cleanup boundary.
        sleep(Duration::from_millis(20)).await;
    };

    match timeout(timeout_duration, close_future).await {
        Ok(_) => {
            tracing::info!(
                component = "lifecycle",
                operation = "close_dependencies",
                status = "success",
                "dependencies closed"
            );
            true
        }
        Err(_) => {
            tracing::warn!(
                component = "lifecycle",
                operation = "close_dependencies",
                status = "timeout",
                "dependency close timed out"
            );
            false
        }
    }
}
