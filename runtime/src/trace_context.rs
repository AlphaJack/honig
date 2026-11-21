//! W3C Trace Context: the `traceparent` and `tracestate` headers.
//!
//! With the `tracing-opentelemetry` feature the request span of the server continues the trace that the `traceparent`
//! of a request names. The requests of a generated client carry the ones of the current span. Without the feature both
//! are no-ops.

use crate::Request;

/// Puts the `traceparent` and `tracestate` of the current span on a request unless it already has a `traceparent`.
/// Generated client methods call it.
#[cfg_attr(not(feature = "tracing-opentelemetry"), expect(clippy::missing_const_for_fn, reason = "a no-op here"))]
pub fn propagate_trace(req: &mut Request) {
    #[cfg(feature = "tracing-opentelemetry")]
    {
        use crate::Message as _;
        use opentelemetry::trace::TraceContextExt;
        use tracing_opentelemetry::OpenTelemetrySpanExt;
        if req.header("traceparent").is_some() {
            return;
        }
        let cx = tracing::Span::current().context();
        let span = cx.span();
        let sc = span.span_context();
        if !sc.is_valid() {
            return;
        }
        // Both are made of characters a header carries.
        req.set_header("traceparent", traceparent(sc)).ok();
        let state = sc.trace_state().header();
        if !state.is_empty() {
            req.set_header("tracestate", state).ok();
        }
    }
    #[cfg(not(feature = "tracing-opentelemetry"))]
    let _ = req;
}

/// The span of the caller that a request identifies in its `traceparent`. It is kept while the request goes through the
/// layers.
#[cfg(feature = "server")]
#[derive(Debug, Default)]
pub(crate) struct Remote(
    #[cfg(all(feature = "tracing", feature = "tracing-opentelemetry"))] Option<opentelemetry::trace::SpanContext>,
);

#[cfg(feature = "server")]
impl Remote {
    /// The span the request's `traceparent` and `tracestate` name.
    #[cfg_attr(
        not(all(feature = "tracing", feature = "tracing-opentelemetry")),
        expect(clippy::missing_const_for_fn, reason = "a no-op here")
    )]
    pub(crate) fn of(headers: &http::HeaderMap) -> Self {
        #[cfg(all(feature = "tracing", feature = "tracing-opentelemetry"))]
        {
            let text = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
            Self(text("traceparent").and_then(|p| span_context(p, text("tracestate"))))
        }
        #[cfg(not(all(feature = "tracing", feature = "tracing-opentelemetry")))]
        {
            let _ = headers;
            Self()
        }
    }

    /// Makes `span` a child of the caller's span.
    #[cfg(feature = "tracing")]
    #[cfg_attr(not(feature = "tracing-opentelemetry"), expect(clippy::unused_self, reason = "a no-op here"))]
    pub(crate) fn adopt(self, span: &tracing::Span) {
        #[cfg(feature = "tracing-opentelemetry")]
        if let Some(sc) = self.0 {
            use opentelemetry::trace::TraceContextExt;
            use tracing_opentelemetry::OpenTelemetrySpanExt;
            // Without an OpenTelemetry layer there is nothing to join.
            span.set_parent(opentelemetry::Context::new().with_remote_span_context(sc)).unwrap_or_default();
        }
        #[cfg(not(feature = "tracing-opentelemetry"))]
        let _ = span;
    }
}

/// Returns a `traceparent` value: version `00`, trace id, parent id and flags in lower-case hex.
#[cfg(feature = "tracing-opentelemetry")]
fn traceparent(sc: &opentelemetry::trace::SpanContext) -> String {
    format!("00-{}-{}-{:02x}", sc.trace_id(), sc.span_id(), sc.trace_flags().to_u8())
}

/// Hex digits of a `traceparent`'s version and flags: one byte each.
#[cfg(all(feature = "tracing", feature = "tracing-opentelemetry"))]
const BYTE_HEX: usize = 2;
/// Hex digits of a trace id: 16 bytes.
#[cfg(all(feature = "tracing", feature = "tracing-opentelemetry"))]
const TRACE_ID_HEX: usize = 32;
/// Hex digits of a parent (span) id: 8 bytes.
#[cfg(all(feature = "tracing", feature = "tracing-opentelemetry"))]
const SPAN_ID_HEX: usize = 16;
/// A `traceparent` after its version: trace id, parent id and flags with a `-` before each of the last two.
#[cfg(all(feature = "tracing", feature = "tracing-opentelemetry"))]
const FIELDS_LEN: usize = TRACE_ID_HEX + 1 + SPAN_ID_HEX + 1 + BYTE_HEX;

/// Returns the span that a `traceparent` names with the `tracestate` next to it. `None` when the `traceparent` is
/// malformed as W3C Trace Context section 3.2 reads it. A future version may append fields after a `-`. `ff` is no
/// version.
#[cfg(all(feature = "tracing", feature = "tracing-opentelemetry"))]
fn span_context(parent: &str, state: Option<&str>) -> Option<opentelemetry::trace::SpanContext> {
    use opentelemetry::trace::{SpanContext, SpanId, TraceFlags, TraceId, TraceState};
    let parent = parent.trim();
    let hex = |s: &str, len: usize| s.len() == len && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    let (version, rest) = parent.split_once('-')?;
    if !hex(version, BYTE_HEX) || version == "ff" {
        return None;
    }
    let fields = rest.get(..FIELDS_LEN)?;
    let tail = rest.get(FIELDS_LEN..)?;
    if (version == "00" && !tail.is_empty()) || !(tail.is_empty() || tail.starts_with('-')) {
        return None;
    }
    let mut parts = fields.split('-');
    let (trace, span, flags) = (parts.next()?, parts.next()?, parts.next()?);
    if !(hex(trace, TRACE_ID_HEX) && hex(span, SPAN_ID_HEX) && hex(flags, BYTE_HEX)) {
        return None;
    }
    let trace = TraceId::from_hex(trace).ok().filter(|t| *t != TraceId::INVALID)?;
    let span = SpanId::from_hex(span).ok().filter(|s| *s != SpanId::INVALID)?;
    let flags = TraceFlags::new(u8::from_str_radix(flags, crate::escape::HEX).ok()? & TraceFlags::SAMPLED.to_u8());
    let state = state.and_then(|s| s.parse::<TraceState>().ok()).unwrap_or_default();
    Some(SpanContext::new(trace, span, flags, true, state))
}

#[cfg(all(test, feature = "tracing-opentelemetry"))]
mod tests {
    use super::*;

    #[cfg(feature = "tracing")]
    const PARENT: &str = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";

    #[cfg(feature = "tracing")]
    #[test]
    fn a_traceparent_reads_back_as_it_was_written() {
        let sc = span_context(PARENT, Some("shop=t61rcWkgMzE")).expect("valid");
        assert!(sc.is_remote() && sc.is_sampled());
        assert_eq!(traceparent(&sc), PARENT);
        assert_eq!(sc.trace_state().header(), "shop=t61rcWkgMzE");
    }

    #[cfg(feature = "tracing")]
    #[test]
    fn a_malformed_traceparent_names_no_span() {
        for bad in [
            "",
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7",
            "ff-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
            "00-4BF92F3577B34DA6A3CE929D0E0E4736-00f067aa0ba902b7-01",
            "00-00000000000000000000000000000000-00f067aa0ba902b7-01",
            "00-4bf92f3577b34da6a3ce929d0e0e4736-0000000000000000-01",
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01-extra",
        ] {
            assert!(span_context(bad, None).is_none(), "{bad}");
        }
    }

    #[cfg(feature = "tracing")]
    #[test]
    fn a_later_version_may_append_fields() {
        let later = "01-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-03-future";
        let sc = span_context(later, None).expect("fields after the fourth are a later version's");
        assert_eq!(sc.trace_flags().to_u8(), 1, "flags this version does not know are dropped");
    }

    #[test]
    fn a_request_without_a_current_span_gets_no_header() {
        let mut req = crate::message::new_request("GET", "/products".to_owned(), vec![]).unwrap();
        propagate_trace(&mut req);
        assert_eq!(crate::Message::header(&req, "traceparent"), None);
    }
}
