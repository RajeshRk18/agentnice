//! The scheduling rule. It has no clock and no I/O, so tests drive it with
//! fixed times.
//!
//! Two rules:
//! - Cap: each priority has a cap. A session at or over its cap in any
//!   window waits until that window resets.
//! - Pace: sessions that start, and sessions that waited, leave one at a
//!   time with a gap between them, highest priority first. Many sessions
//!   that start at once after a reset get "Server is temporarily limiting
//!   requests" errors (claude-code#53922). The gap stops that.

use crate::usage::Window;
use std::{cmp::Reverse, collections::BTreeSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority {
    Low,
    Normal,
    High,
}

impl Priority {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "low" => Some(Self::Low),
            "normal" => Some(Self::Normal),
            "high" => Some(Self::High),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Config {
    /// Percent caps for low, normal and high.
    pub caps: [f64; 3],
    pub gap_ms: u64,
    /// A session that must wait longer than this stops instead.
    pub max_wait_s: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            caps: [80.0, 95.0, 100.0],
            gap_ms: 3000,
            // A 5h window plus some slack, so a 5h wait fits and a 7d wait does not.
            max_wait_s: 5 * 3600 + 600,
        }
    }
}

impl Config {
    fn cap(&self, p: Priority) -> f64 {
        self.caps[p as usize]
    }
}

#[derive(Debug, PartialEq)]
pub enum Step {
    Go,
    /// Ask again at this time (Unix ms), or sooner if woken.
    Wait(u64),
    /// The wait is too long. The session should stop.
    Stop(Window),
}

pub struct Ticket {
    key: (Reverse<Priority>, u64),
    paced: bool,
}

#[derive(Default)]
pub struct Gate {
    /// Waiting paced tickets, highest priority first, then oldest first.
    queue: BTreeSet<(Reverse<Priority>, u64)>,
    next_slot_ms: u64,
    seq: u64,
}

impl Gate {
    /// A paced ticket waits for a slot even when no cap applies.
    pub fn ticket(&mut self, p: Priority, paced: bool) -> Ticket {
        self.seq += 1;
        Ticket {
            key: (Reverse(p), self.seq),
            paced,
        }
    }

    pub fn step(&mut self, cfg: &Config, t: &mut Ticket, usage: &[Window], now_ms: u64) -> Step {
        let now_s = now_ms / 1000;
        if let Some(w) = blocker(cfg, t.key.0.0, usage, now_s) {
            if w.resets_at - now_s > cfg.max_wait_s {
                self.leave(t);
                return Step::Stop(w);
            }
            // It waited, so at the reset it leaves in the paced line.
            t.paced = true;
            self.queue.insert(t.key);
            return Step::Wait(w.resets_at * 1000);
        }
        if !t.paced {
            return Step::Go;
        }
        self.queue.insert(t.key);
        let first_ready = self
            .queue
            .iter()
            .find(|k| blocker(cfg, k.0.0, usage, now_s).is_none());
        if first_ready == Some(&t.key) && now_ms >= self.next_slot_ms {
            self.queue.remove(&t.key);
            self.next_slot_ms = now_ms + cfg.gap_ms;
            return Step::Go;
        }
        Step::Wait(self.next_slot_ms.max(now_ms))
    }

    /// Removes a ticket whose client went away.
    pub fn leave(&mut self, t: &Ticket) {
        self.queue.remove(&t.key);
    }

    pub fn waiting(&self) -> usize {
        self.queue.len()
    }
}

/// The window that holds this priority back, if any. A window whose reset
/// time has passed counts as empty. If several block, the one that resets
/// last decides.
fn blocker(cfg: &Config, p: Priority, usage: &[Window], now_s: u64) -> Option<Window> {
    usage
        .iter()
        .filter(|w| w.resets_at > now_s && w.used >= cfg.cap(p))
        .max_by_key(|w| w.resets_at)
        .copied()
}

#[cfg(test)]
mod tests {
    use super::*;
    use Priority::*;

    const T: u64 = 1_000_000; // seconds
    fn five_hour(used: f64, resets_in_s: u64) -> Window {
        Window {
            name: "five_hour",
            used,
            resets_at: T + resets_in_s,
        }
    }
    fn ms(s: u64) -> u64 {
        s * 1000
    }

    #[test]
    fn low_waits_at_its_cap_and_normal_does_not() {
        let (cfg, mut g) = (Config::default(), Gate::default());
        let usage = [five_hour(85.0, 600)];
        let mut low = g.ticket(Low, false);
        let mut normal = g.ticket(Normal, false);
        assert_eq!(
            g.step(&cfg, &mut low, &usage, ms(T)),
            Step::Wait(ms(T + 600))
        );
        assert_eq!(g.step(&cfg, &mut normal, &usage, ms(T)), Step::Go);
        // At the reset the window counts as empty, and low leaves.
        assert_eq!(g.step(&cfg, &mut low, &usage, ms(T + 600)), Step::Go);
        assert_eq!(g.waiting(), 0);
    }

    #[test]
    fn stops_when_the_reset_is_too_far() {
        let (cfg, mut g) = (Config::default(), Gate::default());
        let week = Window {
            name: "seven_day",
            used: 90.0,
            resets_at: T + 3 * 86400,
        };
        let mut low = g.ticket(Low, false);
        assert_eq!(g.step(&cfg, &mut low, &[week], ms(T)), Step::Stop(week));
        assert_eq!(g.waiting(), 0);
    }

    #[test]
    fn paced_tickets_leave_one_gap_apart() {
        let (cfg, mut g) = (Config::default(), Gate::default());
        let mut a = g.ticket(Normal, true);
        let mut b = g.ticket(Normal, true);
        assert_eq!(g.step(&cfg, &mut a, &[], ms(T)), Step::Go);
        let slot = ms(T) + cfg.gap_ms;
        assert_eq!(g.step(&cfg, &mut b, &[], ms(T)), Step::Wait(slot));
        assert_eq!(g.step(&cfg, &mut b, &[], slot), Step::Go);
    }

    #[test]
    fn high_leaves_before_low_that_came_first() {
        let (cfg, mut g) = (Config::default(), Gate::default());
        let mut first = g.ticket(Normal, true);
        assert_eq!(g.step(&cfg, &mut first, &[], ms(T)), Step::Go);
        let mut low = g.ticket(Low, true);
        let mut high = g.ticket(High, true);
        assert!(matches!(g.step(&cfg, &mut low, &[], ms(T)), Step::Wait(_)));
        assert!(matches!(g.step(&cfg, &mut high, &[], ms(T)), Step::Wait(_)));
        let slot = ms(T) + cfg.gap_ms;
        assert!(matches!(g.step(&cfg, &mut low, &[], slot), Step::Wait(_)));
        assert_eq!(g.step(&cfg, &mut high, &[], slot), Step::Go);
    }

    #[test]
    fn a_capped_ticket_does_not_hold_up_the_line() {
        let (cfg, mut g) = (Config::default(), Gate::default());
        let usage = [five_hour(85.0, 600)];
        let mut low = g.ticket(Low, false);
        assert!(matches!(
            g.step(&cfg, &mut low, &usage, ms(T)),
            Step::Wait(_)
        ));
        let mut normal = g.ticket(Normal, true);
        assert_eq!(g.step(&cfg, &mut normal, &usage, ms(T)), Step::Go);
    }

    #[test]
    fn unpaced_ticket_skips_the_line() {
        let (cfg, mut g) = (Config::default(), Gate::default());
        let mut a = g.ticket(Normal, true);
        let mut b = g.ticket(Normal, true);
        g.step(&cfg, &mut a, &[], ms(T));
        g.step(&cfg, &mut b, &[], ms(T));
        let mut tool = g.ticket(Low, false);
        assert_eq!(g.step(&cfg, &mut tool, &[], ms(T)), Step::Go);
    }
}
