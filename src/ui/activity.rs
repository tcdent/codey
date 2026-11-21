//! The running turn's readout: phase, time since the turn began, tokens so far,
//! with the token count moving steadily instead of jumping.

use std::time::{Duration, Instant};

use crate::llm::Activity;

/// Renders the readout frame by frame, smoothing its token count between arrivals.
#[derive(Default)]
pub struct ActivityReadout {
    tokens: TokenTween,
}

impl ActivityReadout {
    /// The readout for this frame, e.g. `writing 14s · 412 tokens`.
    pub fn text(&mut self, activity: &Activity) -> String {
        let secs = activity.started.elapsed().as_secs();
        let elapsed = if secs < 60 {
            format!("{secs}s")
        } else {
            format!("{}m {:02}s", secs / 60, secs % 60)
        };
        let tokens = activity.tokens.map(|n| self.tokens.show(activity.started, n, Instant::now()));
        match tokens {
            Some(n) if n > 0 => format!("{} {elapsed} · {n} tokens", activity.phase.label()),
            _ => format!("{} {elapsed}", activity.phase.label()),
        }
    }
}

/// The readout's token count, moving steadily instead of jumping as tokens arrive.
///
/// When the count changes, the shown value heads from where it is to the new count
/// over the time the last change took to arrive: it moves at the rate tokens come,
/// trailing the true count by at most one arrival.
#[derive(Default)]
struct TokenTween {
    /// The turn this tween belongs to (its start); a new turn starts over.
    turn: Option<Instant>,
    /// Where the current animation starts and ends, and over what span.
    from: f64,
    to: u32,
    start: Option<Instant>,
    span: Duration,
}

impl TokenTween {
    /// The count to show at `now`, given the turn's true count.
    fn show(&mut self, turn: Instant, tokens: u32, now: Instant) -> u32 {
        if self.turn != Some(turn) {
            // A new turn: start from its count as it stands.
            *self = Self { turn: Some(turn), from: tokens as f64, to: tokens, start: None, span: Duration::ZERO };
        } else if tokens != self.to {
            let shown = self.value(now);
            // The first arrival of a turn has no previous one to time it by: show it.
            self.span = self.start.map_or(Duration::ZERO, |start| now.duration_since(start));
            self.from = if self.start.is_some() { shown } else { tokens as f64 };
            self.to = tokens;
            self.start = Some(now);
        }
        self.value(now).round() as u32
    }

    fn value(&self, now: Instant) -> f64 {
        let Some(start) = self.start else { return self.to as f64 };
        if self.span.is_zero() {
            return self.to as f64;
        }
        let t = (now.duration_since(start).as_secs_f64() / self.span.as_secs_f64()).min(1.0);
        self.from + (self.to as f64 - self.from) * t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_count_moves_at_the_rate_tokens_arrive() {
        let ms = |n| Duration::from_millis(n);
        let turn = Instant::now();
        let mut tween = TokenTween::default();

        assert_eq!(tween.show(turn, 0, turn), 0, "a new turn shows its count");
        assert_eq!(tween.show(turn, 3, turn + ms(900)), 3, "the first arrival has nothing to time it by");

        // 10 more tokens, 100 ms after the last arrival: shown over the next 100 ms.
        assert_eq!(tween.show(turn, 13, turn + ms(1000)), 3);
        assert_eq!(tween.show(turn, 13, turn + ms(1050)), 8);
        assert_eq!(tween.show(turn, 13, turn + ms(1100)), 13);
        assert_eq!(tween.show(turn, 13, turn + ms(1500)), 13, "it stops at the true count");

        // 13 → 23 is timed by the 600 ms since the last arrival. A new arrival 50 ms in
        // carries on from what is shown (13.8), not from the old target, and is timed
        // by its own 50 ms gap.
        assert_eq!(tween.show(turn, 23, turn + ms(1600)), 13);
        assert_eq!(tween.show(turn, 43, turn + ms(1650)), 14);
        assert_eq!(tween.show(turn, 43, turn + ms(1675)), 28);
        assert_eq!(tween.show(turn, 43, turn + ms(1700)), 43);

        let next = turn + ms(5000);
        assert_eq!(tween.show(next, 0, next), 0, "a new turn starts over");
    }
}
