mod agent;
mod config;
mod event;
mod forgejo;
pub mod listeners;
mod relay;
mod state;
mod vcluster;

use clap::Parser;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::info;

#[derive(Parser)]
#[command(name = "pool-manager")]
struct Cli {
    #[arg(long, default_value = "scripts/pool-config.json")]
    config: PathBuf,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let env_path = cli
        .config
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.join(".env"))
        .filter(|p| p.exists());
    if let Some(ref path) = env_path {
        let _ = dotenvy::from_path(path);
    }

    let cfg = config::load(&cli.config)?;
    let state_path = config::resolve_state_path(&cfg.state_dir);
    let json_output = std::env::var("POOL_LOG_JSON").is_ok();

    let _otel_guard = init_tracing(json_output, cfg.otlp_endpoint.as_deref())?;

    info!(
        min_available = cfg.min_available,
        pool_base = cfg.pool_base,
        repo = %cfg.repo,
        otlp = ?cfg.otlp_endpoint,
        "pool-manager starting"
    );

    let bus = event::Bus::new(64);
    let state = state::PoolState::load_or_new(&state_path)?;
    let state = Arc::new(RwLock::new(state));

    ensure_labels(&cfg).await;

    let relay_handle = relay::spawn(bus.clone(), &cfg);
    let reconciler_handle = listeners::spawn_reconciler(bus.clone(), state.clone(), &cfg);
    let listener_handles = listeners::spawn_all(&bus, state.clone(), &cfg);

    info!(
        webhook_port = cfg.webhook_port,
        poll_interval = cfg.poll_interval_secs,
        "pool-manager ready"
    );

    tokio::signal::ctrl_c().await?;
    info!("shutting down...");

    {
        let st = state.read().await;
        let _ = st.save(&state_path);
    }

    bus.publish(event::Event::Shutdown);
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    relay_handle.abort();
    reconciler_handle.abort();
    for h in listener_handles {
        h.abort();
    }

    info!("shutdown complete");
    Ok(())
}

async fn ensure_labels(cfg: &config::Config) {
    let forgejo = forgejo::Forgejo::new(cfg);
    let labels = vec![
        (cfg.labels.ready.clone(), "ready to be claimed by an agent".into()),
        (cfg.labels.in_progress.clone(), "agent is working on this".into()),
        (cfg.labels.review.clone(), "agent is done, awaiting human review".into()),
        (
            cfg.labels.chaos_passed.clone(),
            "the chaos suite passed for the current commits — this work may go to review".into(),
        ),
        (
            cfg.labels.chaos_failed.clone(),
            "the chaos suite failed — see the report comment".into(),
        ),
    ];
    forgejo.ensure_labels(&labels).await;
}

fn init_tracing(
    json: bool,
    otlp_endpoint: Option<&str>,
) -> anyhow::Result<Option<opentelemetry_sdk::trace::TracerProvider>> {
    use opentelemetry::trace::TracerProvider;
    use opentelemetry_otlp::WithExportConfig;
    use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    let fmt = tracing_subscriber::fmt::layer()
        .with_target(false)
        .with_ansi(!json);

    if let Some(endpoint) = otlp_endpoint {
        let span_exporter = opentelemetry_otlp::new_exporter()
            .http()
            .with_endpoint(endpoint)
            .build_span_exporter()?;

        let provider = opentelemetry_sdk::trace::TracerProvider::builder()
            .with_batch_exporter(span_exporter, opentelemetry_sdk::runtime::Tokio)
            .with_config(
                opentelemetry_sdk::trace::Config::default().with_resource(
                    opentelemetry_sdk::Resource::new([opentelemetry::KeyValue::new(
                        "service.name",
                        "pool-manager",
                    )]),
                ),
            )
            .build();

        let tracer = provider.tracer("pool-manager");
        let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);

        tracing_subscriber::registry()
            .with(filter)
            .with(fmt)
            .with(otel_layer)
            .init();

        info!(endpoint, "OTel tracing enabled");
        Ok(Some(provider))
    } else {
        tracing_subscriber::registry().with(filter).with(fmt).init();
        Ok(None)
    }
}

