//! Prometheus metrics through the `metrics` facade: any module records with
//! `metrics::counter!` / `gauge!` / `histogram!` and `/metrics` renders them. The spec section 12
//! names (`pascal_order_latency_ms`, `pending_open`, ...) are recorded by the L2 services that
//! own them; the shell records `desk_*` metrics.

use std::sync::OnceLock;

use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};

static HANDLE: OnceLock<Option<PrometheusHandle>> = OnceLock::new();

/// Installs the global recorder once per process (tests start several desks in one process).
/// Returns `false` when another recorder was installed first.
pub fn install() -> bool {
    HANDLE
        .get_or_init(|| {
            let recorder = PrometheusBuilder::new().build_recorder();
            let handle = recorder.handle();
            match metrics::set_global_recorder(recorder) {
                Ok(()) => Some(handle),
                Err(e) => {
                    tracing::warn!(error = %e, "metrics recorder already installed; /metrics is empty");
                    None
                }
            }
        })
        .is_some()
}

/// The exposition text (empty before [`install`]).
pub fn render() -> String {
    HANDLE
        .get()
        .and_then(Option::as_ref)
        .map(PrometheusHandle::render)
        .unwrap_or_default()
}

/// Housekeeping for histograms and summaries; called periodically by the heartbeat task.
pub fn upkeep() {
    if let Some(Some(h)) = HANDLE.get() {
        h.run_upkeep();
    }
}
