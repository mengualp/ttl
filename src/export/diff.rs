//! Session diff: compare two saved traces and report path/latency changes.

use crate::lookup::sanitize_display;
use crate::state::{Hop, Session};
use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::io::Write;
use std::net::IpAddr;

/// Minimum absolute avg RTT shift (ms) considered significant
const RTT_ABS_THRESHOLD_MS: f64 = 5.0;
/// Minimum relative avg RTT shift (fraction of the before value) considered significant
const RTT_REL_THRESHOLD: f64 = 0.20;

/// How a hop changed between the two sessions
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HopChange {
    /// Responds in both sessions with the same primary responder
    Unchanged,
    /// Primary responder differs between sessions
    PathChange,
    /// Responds only in the after session
    Added,
    /// Responds only in the before session
    Lost,
}

/// Snapshot of a responding hop in one session
#[derive(Debug, Clone, Serialize)]
pub struct HopSnapshot {
    pub primary: Option<IpAddr>,
    pub hostname: Option<String>,
    pub avg_rtt_ms: Option<f64>,
    pub loss_pct: f64,
    /// All responder IPs seen at this hop (sorted)
    pub responders: Vec<IpAddr>,
}

impl HopSnapshot {
    /// Build a snapshot if the hop responded at least once
    fn from_hop(hop: &Hop) -> Option<Self> {
        if hop.received == 0 {
            return None;
        }
        let primary_stats = hop.primary_stats();
        let mut responders: Vec<IpAddr> = hop.responders.keys().copied().collect();
        responders.sort();
        Some(Self {
            primary: hop.primary,
            hostname: primary_stats.and_then(|s| s.hostname.clone()),
            avg_rtt_ms: primary_stats.map(|s| s.avg_rtt().as_secs_f64() * 1000.0),
            loss_pct: hop.loss_pct(),
            responders,
        })
    }
}

/// Diff of a single TTL between two sessions
#[derive(Debug, Clone, Serialize)]
pub struct HopDiff {
    pub ttl: u8,
    pub change: HopChange,
    pub before: Option<HopSnapshot>,
    pub after: Option<HopSnapshot>,
    /// after - before, when both sessions have RTT data
    pub avg_rtt_delta_ms: Option<f64>,
    /// after - before, when the hop responds in both sessions
    pub loss_delta_pct: Option<f64>,
    /// Whether the RTT shift exceeds both absolute (5ms) and relative (20%) thresholds
    pub rtt_significant: bool,
    pub responders_added: Vec<IpAddr>,
    pub responders_removed: Vec<IpAddr>,
}

/// Complete diff between two sessions
#[derive(Debug, Clone, Serialize)]
pub struct SessionDiff {
    pub before_file: String,
    pub after_file: String,
    pub target_before: String,
    pub target_after: String,
    pub started_before: DateTime<Utc>,
    pub started_after: DateTime<Utc>,
    pub complete_before: bool,
    pub complete_after: bool,
    pub dest_ttl_before: Option<u8>,
    pub dest_ttl_after: Option<u8>,
    /// Per-TTL diffs, only for hops that responded in at least one session
    pub hops: Vec<HopDiff>,
}

impl SessionDiff {
    pub fn count(&self, change: HopChange) -> usize {
        self.hops.iter().filter(|h| h.change == change).count()
    }

    pub fn rtt_shifts(&self) -> usize {
        self.hops.iter().filter(|h| h.rtt_significant).count()
    }

    /// True if anything changed: path, hop presence, or significant latency
    pub fn has_changes(&self) -> bool {
        self.rtt_shifts() > 0
            || self.hops.iter().any(|h| {
                h.change != HopChange::Unchanged
                    || !h.responders_added.is_empty()
                    || !h.responders_removed.is_empty()
            })
    }
}

/// Compare two sessions hop by hop
pub fn diff_sessions(
    before: &Session,
    after: &Session,
    before_file: &str,
    after_file: &str,
) -> SessionDiff {
    let max_len = before.hops.len().max(after.hops.len());
    let mut hops = Vec::new();

    for i in 0..max_len {
        let snap_before = before.hops.get(i).and_then(HopSnapshot::from_hop);
        let snap_after = after.hops.get(i).and_then(HopSnapshot::from_hop);
        let ttl = (i + 1) as u8;

        let change = match (&snap_before, &snap_after) {
            (None, None) => continue, // silent in both: nothing to report
            (None, Some(_)) => HopChange::Added,
            (Some(_), None) => HopChange::Lost,
            (Some(b), Some(a)) => {
                if b.primary == a.primary {
                    HopChange::Unchanged
                } else {
                    HopChange::PathChange
                }
            }
        };

        let avg_rtt_delta_ms = match (&snap_before, &snap_after) {
            (Some(b), Some(a)) => match (b.avg_rtt_ms, a.avg_rtt_ms) {
                (Some(rb), Some(ra)) => Some(ra - rb),
                _ => None,
            },
            _ => None,
        };
        let loss_delta_pct = match (&snap_before, &snap_after) {
            (Some(b), Some(a)) => Some(a.loss_pct - b.loss_pct),
            _ => None,
        };
        let rtt_significant = match (avg_rtt_delta_ms, &snap_before) {
            (Some(delta), Some(b)) => {
                let base = b.avg_rtt_ms.unwrap_or(0.0);
                delta.abs() >= RTT_ABS_THRESHOLD_MS && delta.abs() >= base * RTT_REL_THRESHOLD
            }
            _ => false,
        };

        let (responders_added, responders_removed) = match (&snap_before, &snap_after) {
            (Some(b), Some(a)) => (
                a.responders
                    .iter()
                    .filter(|ip| !b.responders.contains(ip))
                    .copied()
                    .collect(),
                b.responders
                    .iter()
                    .filter(|ip| !a.responders.contains(ip))
                    .copied()
                    .collect(),
            ),
            _ => (Vec::new(), Vec::new()),
        };

        hops.push(HopDiff {
            ttl,
            change,
            before: snap_before,
            after: snap_after,
            avg_rtt_delta_ms,
            loss_delta_pct,
            rtt_significant,
            responders_added,
            responders_removed,
        });
    }

    SessionDiff {
        before_file: before_file.to_string(),
        after_file: after_file.to_string(),
        target_before: before.target.display_name(),
        target_after: after.target.display_name(),
        started_before: before.started_at,
        started_after: after.started_at,
        complete_before: before.complete,
        complete_after: after.complete,
        dest_ttl_before: before.dest_ttl,
        dest_ttl_after: after.dest_ttl,
        hops,
    }
}

/// ANSI color codes (only applied when `color` is true)
struct Palette {
    color: bool,
}

impl Palette {
    fn paint(&self, code: &str, s: &str) -> String {
        if self.color {
            format!("\x1b[{}m{}\x1b[0m", code, s)
        } else {
            s.to_string()
        }
    }
    fn green(&self, s: &str) -> String {
        self.paint("32", s)
    }
    fn red(&self, s: &str) -> String {
        self.paint("31", s)
    }
    fn yellow(&self, s: &str) -> String {
        self.paint("33", s)
    }
    fn dim(&self, s: &str) -> String {
        self.paint("2", s)
    }
}

/// Format a hop's display name: hostname if known, else primary IP, else "*"
fn host_label(snap: &Option<HopSnapshot>) -> String {
    match snap {
        Some(s) => match (&s.hostname, s.primary) {
            (Some(h), _) => sanitize_display(h),
            (None, Some(ip)) => ip.to_string(),
            (None, None) => "*".to_string(),
        },
        None => "*".to_string(),
    }
}

fn fmt_rtt(v: Option<f64>) -> String {
    match v {
        Some(ms) => format!("{:>7.1}", ms),
        None => format!("{:>7}", "-"),
    }
}

fn fmt_loss(snap: &Option<HopSnapshot>) -> String {
    match snap {
        Some(s) => format!("{:>5.1}", s.loss_pct),
        None => format!("{:>5}", "-"),
    }
}

/// Write a human-readable diff report
pub fn write_diff_text<W: Write>(diff: &SessionDiff, mut w: W, color: bool) -> Result<()> {
    let p = Palette { color };

    let target = if diff.target_before == diff.target_after {
        sanitize_display(&diff.target_before)
    } else {
        format!(
            "{} vs {}",
            sanitize_display(&diff.target_before),
            sanitize_display(&diff.target_after)
        )
    };
    writeln!(w, "Trace diff: {}", target)?;
    writeln!(
        w,
        "  before: {}  ({})",
        sanitize_display(&diff.before_file),
        diff.started_before.format("%Y-%m-%d %H:%M:%S UTC")
    )?;
    writeln!(
        w,
        "  after:  {}  ({})",
        sanitize_display(&diff.after_file),
        diff.started_after.format("%Y-%m-%d %H:%M:%S UTC")
    )?;
    writeln!(w)?;

    // Host column width: fits the longest label on either side
    let host_w = diff
        .hops
        .iter()
        .flat_map(|h| {
            [
                host_label(&h.before).chars().count(),
                host_label(&h.after).chars().count(),
            ]
        })
        .max()
        .unwrap_or(15)
        .max(15);

    writeln!(
        w,
        " TTL  {:<hw$}  {:<hw$}  {:<8}  {:>15}  {:>13}",
        "Before",
        "After",
        "Change",
        "Avg RTT (ms)",
        "Loss (%)",
        hw = host_w
    )?;

    for hop in &diff.hops {
        let (marker, painted_marker) = match hop.change {
            HopChange::Unchanged => ("", String::new()),
            HopChange::PathChange => ("[path]", p.yellow("[path]")),
            HopChange::Added => ("[added]", p.green("[added]")),
            HopChange::Lost => ("[lost]", p.red("[lost]")),
        };
        // Pad based on the unpainted marker (ANSI codes have zero display width)
        let marker_cell = format!("{}{}", painted_marker, " ".repeat(8 - marker.len()));

        let rtt_cell = {
            let cell = format!(
                "{} \u{2192} {}",
                fmt_rtt(hop.before.as_ref().and_then(|s| s.avg_rtt_ms)),
                fmt_rtt(hop.after.as_ref().and_then(|s| s.avg_rtt_ms))
            );
            if hop.rtt_significant {
                p.yellow(&cell)
            } else {
                cell
            }
        };
        let loss_cell = format!(
            "{} \u{2192} {}",
            fmt_loss(&hop.before),
            fmt_loss(&hop.after)
        );

        let before_label = host_label(&hop.before);
        let after_label = host_label(&hop.after);
        writeln!(
            w,
            " {:>3}  {:<hw$}  {:<hw$}  {}  {}  {}",
            hop.ttl,
            before_label,
            after_label,
            marker_cell,
            rtt_cell,
            loss_cell,
            hw = host_w
        )?;

        // ECMP responder-set changes (beyond the primary)
        if !hop.responders_added.is_empty() || !hop.responders_removed.is_empty() {
            let mut parts = Vec::new();
            for ip in &hop.responders_added {
                parts.push(p.green(&format!("+{}", ip)));
            }
            for ip in &hop.responders_removed {
                parts.push(p.red(&format!("-{}", ip)));
            }
            writeln!(w, "      responders: {}", parts.join("  "))?;
        }
    }

    writeln!(w)?;

    let path_changes = diff.count(HopChange::PathChange);
    let added = diff.count(HopChange::Added);
    let lost = diff.count(HopChange::Lost);
    let rtt_shifts = diff.rtt_shifts();

    if diff.has_changes() {
        writeln!(
            w,
            "Summary: {} path change(s), {} hop(s) added, {} hop(s) lost, {} significant latency shift(s)",
            path_changes, added, lost, rtt_shifts
        )?;
    } else {
        writeln!(w, "Summary: {}", p.dim("no changes detected"))?;
    }

    let dest = match (diff.complete_before, diff.complete_after) {
        (true, true) => {
            let hops_before = diff
                .dest_ttl_before
                .map(|t| t.to_string())
                .unwrap_or_else(|| "?".into());
            let hops_after = diff
                .dest_ttl_after
                .map(|t| t.to_string())
                .unwrap_or_else(|| "?".into());
            format!(
                "destination reached in both ({} \u{2192} {} hops)",
                hops_before, hops_after
            )
        }
        (true, false) => p.red("destination reached only in before session"),
        (false, true) => p.green("destination reached only in after session"),
        (false, false) => p.dim("destination not reached in either session"),
    };
    writeln!(w, "  {}", dest)?;

    Ok(())
}

/// Write the diff as pretty-printed JSON
pub fn write_diff_json<W: Write>(diff: &SessionDiff, writer: W) -> Result<()> {
    serde_json::to_writer_pretty(writer, diff)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::state::Target;
    use std::net::Ipv4Addr;
    use std::time::Duration;

    fn ip(last: u8) -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(10, 0, 0, last))
    }

    fn session_with_hops(hops: &[(u8, IpAddr, u64)]) -> Session {
        let target = Target::new("test.example".to_string(), ip(99));
        let mut session = Session::new(target, Config::default());
        for &(ttl, addr, rtt_ms) in hops {
            if let Some(hop) = session.hop_mut(ttl) {
                hop.record_sent();
                hop.record_response(addr, Duration::from_millis(rtt_ms));
            }
        }
        session
    }

    #[test]
    fn test_diff_no_changes() {
        let a = session_with_hops(&[(1, ip(1), 5), (2, ip(2), 10)]);
        let b = session_with_hops(&[(1, ip(1), 5), (2, ip(2), 10)]);
        let diff = diff_sessions(&a, &b, "a.json", "b.json");
        assert_eq!(diff.hops.len(), 2);
        assert!(!diff.has_changes());
        assert!(
            diff.hops
                .iter()
                .all(|h| h.change == HopChange::Unchanged && !h.rtt_significant)
        );
    }

    #[test]
    fn test_diff_path_change() {
        let a = session_with_hops(&[(1, ip(1), 5), (2, ip(2), 10)]);
        let b = session_with_hops(&[(1, ip(1), 5), (2, ip(3), 10)]);
        let diff = diff_sessions(&a, &b, "a.json", "b.json");
        assert_eq!(diff.count(HopChange::PathChange), 1);
        let changed = &diff.hops[1];
        assert_eq!(changed.ttl, 2);
        assert_eq!(changed.change, HopChange::PathChange);
        assert_eq!(changed.responders_added, vec![ip(3)]);
        assert_eq!(changed.responders_removed, vec![ip(2)]);
    }

    #[test]
    fn test_diff_added_and_lost_hops() {
        let a = session_with_hops(&[(1, ip(1), 5), (3, ip(3), 15)]);
        let b = session_with_hops(&[(1, ip(1), 5), (2, ip(2), 10)]);
        let diff = diff_sessions(&a, &b, "a.json", "b.json");
        assert_eq!(diff.count(HopChange::Added), 1);
        assert_eq!(diff.count(HopChange::Lost), 1);
        let added = diff.hops.iter().find(|h| h.ttl == 2).unwrap();
        assert_eq!(added.change, HopChange::Added);
        assert!(added.before.is_none());
        let lost = diff.hops.iter().find(|h| h.ttl == 3).unwrap();
        assert_eq!(lost.change, HopChange::Lost);
        assert!(lost.after.is_none());
    }

    #[test]
    fn test_diff_skips_silent_hops() {
        let a = session_with_hops(&[(1, ip(1), 5), (5, ip(5), 25)]);
        let b = session_with_hops(&[(1, ip(1), 5), (5, ip(5), 25)]);
        let diff = diff_sessions(&a, &b, "a.json", "b.json");
        // TTLs 2-4 never responded in either session: not reported
        let ttls: Vec<u8> = diff.hops.iter().map(|h| h.ttl).collect();
        assert_eq!(ttls, vec![1, 5]);
    }

    #[test]
    fn test_diff_rtt_significance() {
        // 10ms -> 100ms: significant (abs 90ms >= 5ms, rel 900% >= 20%)
        let a = session_with_hops(&[(1, ip(1), 10)]);
        let b = session_with_hops(&[(1, ip(1), 100)]);
        let diff = diff_sessions(&a, &b, "a.json", "b.json");
        assert!(diff.hops[0].rtt_significant);
        assert!((diff.hops[0].avg_rtt_delta_ms.unwrap() - 90.0).abs() < 0.5);
        assert!(diff.has_changes());

        // 100ms -> 104ms: not significant (abs 4ms < 5ms)
        let a = session_with_hops(&[(1, ip(1), 100)]);
        let b = session_with_hops(&[(1, ip(1), 104)]);
        let diff = diff_sessions(&a, &b, "a.json", "b.json");
        assert!(!diff.hops[0].rtt_significant);

        // 100ms -> 110ms: not significant (rel 10% < 20%)
        let a = session_with_hops(&[(1, ip(1), 100)]);
        let b = session_with_hops(&[(1, ip(1), 110)]);
        let diff = diff_sessions(&a, &b, "a.json", "b.json");
        assert!(!diff.hops[0].rtt_significant);
    }

    #[test]
    fn test_diff_ecmp_responder_set_change() {
        let mut a = session_with_hops(&[(1, ip(1), 5)]);
        if let Some(hop) = a.hop_mut(1) {
            hop.record_sent();
            hop.record_response(ip(1), Duration::from_millis(5));
            hop.record_sent();
            hop.record_response(ip(7), Duration::from_millis(6));
        }
        let b = session_with_hops(&[(1, ip(1), 5)]);
        let diff = diff_sessions(&a, &b, "a.json", "b.json");
        // Primary unchanged but responder 10.0.0.7 disappeared
        assert_eq!(diff.hops[0].change, HopChange::Unchanged);
        assert_eq!(diff.hops[0].responders_removed, vec![ip(7)]);
        assert!(diff.has_changes());
    }

    #[test]
    fn test_diff_text_output() {
        let a = session_with_hops(&[(1, ip(1), 5), (2, ip(2), 10)]);
        let b = session_with_hops(&[(1, ip(1), 5), (2, ip(3), 10), (3, ip(4), 20)]);
        let diff = diff_sessions(&a, &b, "a.json", "b.json");
        let mut out = Vec::new();
        write_diff_text(&diff, &mut out, false).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("Trace diff: test.example"));
        assert!(text.contains("[path]"));
        assert!(text.contains("[added]"));
        assert!(text.contains("1 path change(s), 1 hop(s) added, 0 hop(s) lost"));
        // No ANSI escapes when color is off
        assert!(!text.contains('\x1b'));
    }

    #[test]
    fn test_diff_json_output_roundtrips() {
        let a = session_with_hops(&[(1, ip(1), 5)]);
        let b = session_with_hops(&[(1, ip(2), 5)]);
        let diff = diff_sessions(&a, &b, "a.json", "b.json");
        let mut out = Vec::new();
        write_diff_json(&diff, &mut out).unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(parsed["hops"][0]["change"], "path_change");
    }
}
