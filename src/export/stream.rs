//! Line-delimited JSON event streaming for --stream-json mode.
//!
//! Each probe event is emitted as one JSON object per line, matching the
//! event schema used in saved session files (offset_ms/ttl/seq/flow_id plus
//! a flattened outcome tagged by "type"), with the target IP added so
//! multi-target streams can be demultiplexed with jq/grep.

use crate::state::{ProbeEvent, Session};
use anyhow::Result;
use serde::Serialize;
use std::io::Write;
use std::net::IpAddr;

/// A probe event tagged with its target
#[derive(Serialize)]
struct EventLine<'a> {
    target: IpAddr,
    #[serde(flatten)]
    event: &'a ProbeEvent,
}

/// Per-target summary emitted when the stream ends
#[derive(Serialize)]
struct SummaryLine {
    target: IpAddr,
    #[serde(rename = "type")]
    kind: &'static str,
    complete: bool,
    dest_ttl: Option<u8>,
    total_sent: u64,
    hops_responding: usize,
}

/// Write one probe event as a JSON line
pub fn write_event_line<W: Write>(mut w: W, target: IpAddr, event: &ProbeEvent) -> Result<()> {
    serde_json::to_writer(&mut w, &EventLine { target, event })?;
    writeln!(w)?;
    Ok(())
}

/// Write the end-of-stream summary for a target as a JSON line
pub fn write_summary_line<W: Write>(mut w: W, target: IpAddr, session: &Session) -> Result<()> {
    let line = SummaryLine {
        target,
        kind: "summary",
        complete: session.complete,
        dest_ttl: session.dest_ttl,
        total_sent: session.total_sent,
        hops_responding: session.hops.iter().filter(|h| h.received > 0).count(),
    };
    serde_json::to_writer(&mut w, &line)?;
    writeln!(w)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::state::{ProbeOutcome, Target};
    use std::net::Ipv4Addr;

    fn target_ip() -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))
    }

    #[test]
    fn test_event_line_reply_schema() {
        let event = ProbeEvent {
            offset_ms: 1234,
            ttl: 5,
            seq: 2,
            flow_id: 0,
            outcome: ProbeOutcome::Reply {
                addr: IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
                rtt_us: 12500,
            },
        };
        let mut out = Vec::new();
        write_event_line(&mut out, target_ip(), &event).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.ends_with('\n'));
        assert_eq!(text.lines().count(), 1);
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["target"], "8.8.8.8");
        assert_eq!(v["type"], "reply");
        assert_eq!(v["ttl"], 5);
        assert_eq!(v["offset_ms"], 1234);
        assert_eq!(v["addr"], "10.0.0.1");
        assert_eq!(v["rtt_us"], 12500);
    }

    #[test]
    fn test_event_line_timeout_schema() {
        let event = ProbeEvent {
            offset_ms: 5000,
            ttl: 3,
            seq: 7,
            flow_id: 1,
            outcome: ProbeOutcome::Timeout,
        };
        let mut out = Vec::new();
        write_event_line(&mut out, target_ip(), &event).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["type"], "timeout");
        assert_eq!(v["flow_id"], 1);
        assert!(v.get("addr").is_none());
    }

    #[test]
    fn test_summary_line_schema() {
        let target = Target::new("8.8.8.8".to_string(), target_ip());
        let mut session = Session::new(target, Config::default());
        session.complete = true;
        session.dest_ttl = Some(13);
        session.total_sent = 42;
        let mut out = Vec::new();
        write_summary_line(&mut out, target_ip(), &session).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["type"], "summary");
        assert_eq!(v["complete"], true);
        assert_eq!(v["dest_ttl"], 13);
        assert_eq!(v["total_sent"], 42);
        assert_eq!(v["hops_responding"], 0);
    }
}
