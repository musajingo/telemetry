//! HTTP request tracing that continues an upstream distributed trace.

mod make_span;
mod on_response;
mod trace_layer;

pub use make_span::OtelMakeSpan;
pub(crate) use make_span::set_trust_incoming_trace;
pub use on_response::OtelOnResponse;
pub use trace_layer::http_trace_layer;
