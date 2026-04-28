use vm_agent::agent::registry as agent_registry;
use vm_agent::app::context::AppContext;
use vm_agent::app::lifecycle::{self, ShutdownResult};
use vm_agent::app::state::AppState;
use vm_agent::config::load_from_env;
use vm_agent::telemetry::logging::{app_span, init};

#[tokio::main]
async fn main() {
    // Install the default crypto provider for rustls 0.23+
    rustls::crypto::ring::default_provider()
        .install_default()
        .ok();

    if std::env::args().nth(1).as_deref() == Some("--print-node-id") {
        println!("{}", ulid::Ulid::new());
        return;
    }

    let config = match load_from_env() {
        Ok(cfg) => cfg,
        Err(err) => {
            eprintln!("fatal bootstrap error: invalid configuration: {err}");
            std::process::exit(1);
        }
    };

    let _logging_guard = match init(&config.log) {
        Ok(guard) => guard,
        Err(err) => {
            // Early bootstrap path before logger init.
            eprintln!("fatal bootstrap error: failed to initialize logger: {err}");
            std::process::exit(1);
        }
    };

    let app_scope = app_span(&config.log);
    let _entered = app_scope.enter();

    let ctx = AppContext::new(config.clone());
    ctx.state.set(AppState::Running);

    tracing::info!(
        component = "bootstrap",
        operation = "process_start",
        status = "success",
        "vm-agent process started"
    );
    tracing::info!(
        component = "bootstrap",
        operation = "config_load",
        status = "success",
        environment = %config.log.environment,
        grpc_bind_addr = %config.grpc.bind_addr,
        worker_max = config.worker.max_workers as u64,
        "configuration loaded"
    );

    {
        let grpc_server = ctx.grpc_server.clone();
        let shutdown = ctx.shutdown_token.clone();
        ctx.tasks.spawn(async move {
            grpc_server.run(shutdown).await;
        });
    }

    {
        let dispatcher = ctx.dispatcher.clone();
        let pool = ctx.worker_pool.clone();
        let shutdown = ctx.shutdown_token.clone();
        ctx.tasks.spawn(async move {
            dispatcher.run(shutdown, pool).await;
        });
    }

    {
        let config = ctx.config.clone();
        let shutdown = ctx.shutdown_token.clone();
        ctx.tasks.spawn(async move {
            if let Err(err) = agent_registry::connect_hypervisor(config, shutdown).await {
                tracing::error!(
                    component = "bootstrap",
                    operation = "agent_enrollment",
                    status = "error",
                    error_code = "AGENT_CONNECT_FAILED",
                    error_message = %err,
                    "agent hypervisor enrollment loop failed"
                );
            }
        });
    }

    match lifecycle::wait_for_shutdown_signal().await {
        Ok(signal) => {
            tracing::info!(
                component = "bootstrap",
                operation = "shutdown_trigger",
                status = "received",
                signal = %signal,
                "shutdown signal received, starting graceful shutdown"
            );
        }
        Err(err) => {
            tracing::error!(
                component = "bootstrap",
                operation = "shutdown_trigger",
                status = "error",
                error_code = "SIGNAL_WAIT_FAILED",
                error_message = %err,
                "failed to listen for shutdown signal, forcing shutdown"
            );
        }
    }

    let shutdown_result = lifecycle::shutdown(&ctx, ctx.shutdown_timeout).await;
    match shutdown_result {
        ShutdownResult::Clean => {
            tracing::info!(
                component = "bootstrap",
                operation = "process_shutdown",
                status = "success",
                "vm-agent process shutdown cleanly"
            );
        }
        ShutdownResult::TimedOut => {
            tracing::warn!(
                component = "bootstrap",
                operation = "process_shutdown",
                status = "timeout",
                "vm-agent process shutdown forced by timeout"
            );
        }
    }

    tracing::info!(
        component = "bootstrap",
        operation = "process_exit",
        status = "success",
        "vm-agent exiting"
    );
}
