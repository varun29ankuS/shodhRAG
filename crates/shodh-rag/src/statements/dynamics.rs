//! Memory dynamics: decay, reinforcement, expiry, importance, Hebbian links and spreading
//! activation. Pure functions of their inputs (time is always passed in), so every rule is
//! testable with an injected clock and nothing here touches storage.
//!
//! The algorithms are reimplemented from the Apache-2.0 project shodh-memory:
//! - hybrid exponential → power-law forgetting curve: `src/decay.rs`
//!   (`hybrid_decay_factor_scaled`), constants in `src/constants.rs`
//!   (`DECAY_CROSSOVER_DAYS`, `POWERLAW_BETA`, `POWERLAW_BETA_POTENTIATED`);
//! - Hebbian co-activation strengthening `w += η·(1 − w)·scale`: `src/graph_memory.rs`
//!   (`RelationshipEdge::strengthen_scaled_at`), constants `LTP_LEARNING_RATE`,
//!   `STRENGTHEN_IMPORTANCE_FLOOR`, `LTP_DECAY_HALF_LIFE_DAYS`, `LTP_THRESHOLD`;
//! - spreading activation with per-hop exponential decay and fan-out (degree)
//!   normalisation: `src/memory/graph_retrieval.rs` (`spread_single_direction`), constants
//!   `SPREADING_DECAY_RATE`, `SPREADING_ACTIVATION_THRESHOLD`, `ACTIVATION_BONUS_SCALE`;
//! - multi-factor importance: `src/memory/mod.rs` (`calculate_importance`).
//!
//! Adaptation: shodh-memory's curve has a fixed one-day consolidation half-life. Here every
//! ontology class declares its own `decay_half_life_days`, so time is measured in
//! half-lives of the class (the same time-axis scaling shodh-memory uses for its semantic
//! tier, `L3_TIME_SCALE_VS_L2`). The exponential leg therefore honours the declared half-life
//! exactly, and after three half-lives the tail becomes a power law.

use std::collections::{BTreeMap, HashMap};

use chrono::{DateTime, NaiveTime, TimeZone, Utc};
use shodh_ontology::{Dynamics, Expiry, ExtractorKind, Ontology, ValidStatement, Value};

/// Crossover from the exponential (consolidation) leg to the power-law tail, in half-lives.
/// shodh-memory: `DECAY_CROSSOVER_DAYS = 3` with a one-day consolidation half-life.
pub const CROSSOVER_HALF_LIVES: f64 = 3.0;

/// Power-law exponent of the long-term tail (`POWERLAW_BETA`).
pub const POWER_LAW_BETA: f64 = 0.5;

/// Power-law exponent for potentiated (much-used) memories: a heavier tail
/// (`POWERLAW_BETA_POTENTIATED`).
pub const POWER_LAW_BETA_POTENTIATED: f64 = 0.3;

/// Uses after which a memory or link counts as potentiated (`LTP_THRESHOLD`). Potentiated
/// items also decay at half the exponential rate, as in shodh-memory.
pub const POTENTIATION_USES: u32 = 10;

/// Hebbian learning rate η (`LTP_LEARNING_RATE`): about ten co-activations saturate a link.
pub const HEBBIAN_LEARNING_RATE: f64 = 0.1;

/// Fraction of the full Hebbian boost that the least important memory still gets
/// (`STRENGTHEN_IMPORTANCE_FLOOR`), so unimportant memories are not starved of links.
pub const HEBBIAN_IMPORTANCE_FLOOR: f64 = 0.2;

/// Half-life of an unused link, in days (`LTP_DECAY_HALF_LIFE_DAYS`).
pub const LINK_HALF_LIFE_DAYS: f64 = 14.0;

/// Links weaker than this (after decay) are dropped when a statement's links are rewritten.
pub const LINK_PRUNE_FLOOR: f64 = 0.01;

/// At most this many links are kept per statement; the weakest go first. Bounds both the
/// table and the fan-out of spreading activation.
pub const MAX_LINKS_PER_STATEMENT: usize = 32;

/// At most this many co-recalled statements strengthen links in one recall (28 pairs).
pub const MAX_CO_RECALLED: usize = 8;

/// Per-hop decay rate λ of spreading activation, `A(d) = A₀·e^(−λd)`
/// (`SPREADING_DECAY_RATE`).
pub const SPREADING_DECAY_RATE: f64 = 0.5;

/// Activation below which a node stops spreading (`SPREADING_ACTIVATION_THRESHOLD`).
pub const SPREADING_THRESHOLD: f64 = 0.005;

/// Hops of spreading activation. Memory graphs are small and shallow; two hops reach
/// "associated with something associated", which is as far as an association stays
/// meaningful for recall.
pub const SPREADING_HOPS: usize = 2;

/// Floor of the importance score (`IMPORTANCE_FLOOR`).
pub const IMPORTANCE_FLOOR: f64 = 0.05;

const DAY_SECONDS: f64 = 86_400.0;

/// Days from `from` to `to` (negative when `to` is earlier).
pub fn days_between(from: DateTime<Utc>, to: DateTime<Utc>) -> f64 {
    (to - from).num_milliseconds() as f64 / 1000.0 / DAY_SECONDS
}

/// Fraction of strength retained after `age_days` for a class with the given half-life.
///
/// `None` (no decay) and non-positive ages retain everything. With `t = age / half_life`:
/// - `t < 3`: `e^(−λt)` with `λ = ln 2` (halved when potentiated), so `t = 1` gives exactly
///   one half for an ordinary memory;
/// - `t ≥ 3`: `e^(−3λ) · (t / 3)^(−β)`, continuous at the crossover, with `β = 0.5`
///   (`0.3` when potentiated).
pub fn retention(age_days: f64, half_life_days: Option<f64>, potentiated: bool) -> f64 {
    let Some(half_life) = half_life_days.filter(|h| *h > 0.0 && h.is_finite()) else {
        return 1.0;
    };
    if age_days.is_nan() || age_days <= 0.0 {
        return 1.0;
    }
    if age_days.is_infinite() {
        return 0.0;
    }
    let t = age_days / half_life;
    let (lambda, beta) = if potentiated {
        (std::f64::consts::LN_2 * 0.5, POWER_LAW_BETA_POTENTIATED)
    } else {
        (std::f64::consts::LN_2, POWER_LAW_BETA)
    };
    if t < CROSSOVER_HALF_LIVES {
        (-lambda * t).exp()
    } else {
        (-lambda * CROSSOVER_HALF_LIVES).exp() * (t / CROSSOVER_HALF_LIVES).powf(-beta)
    }
}

/// Stored recall state of one statement. Strength is the value at `anchor_at`; the current
/// value is computed lazily by [`DynamicsState::strength_at`], so reads never write.
#[derive(Debug, Clone, PartialEq)]
pub struct DynamicsState {
    /// Strength at `anchor_at`, in `[0, 1]`.
    pub strength: f64,
    /// When `strength` was last set (creation, reinforcement or pinning).
    pub anchor_at: DateTime<Utc>,
    /// Importance in `[IMPORTANCE_FLOOR, 1]`, fixed at write time.
    pub importance: f64,
    /// Times the statement was recalled and used.
    pub use_count: u32,
    /// Last time it was recalled and used.
    pub last_used_at: Option<DateTime<Utc>>,
    /// Exempt from decay.
    pub pinned: bool,
}

impl DynamicsState {
    /// A fresh statement: full strength at `at`.
    pub fn fresh(at: DateTime<Utc>, importance: f64) -> Self {
        Self {
            strength: 1.0,
            anchor_at: at,
            importance: importance.clamp(IMPORTANCE_FLOOR, 1.0),
            use_count: 0,
            last_used_at: None,
            pinned: false,
        }
    }

    /// Whether repeated use has potentiated the memory (slower decay, heavier tail).
    pub fn potentiated(&self) -> bool {
        self.use_count >= POTENTIATION_USES
    }

    /// Strength at `now` under the class dynamics. Pinned statements do not decay.
    pub fn strength_at(&self, dynamics: &Dynamics, now: DateTime<Utc>) -> f64 {
        if self.pinned {
            return self.strength.clamp(0.0, 1.0);
        }
        let age = days_between(self.anchor_at, now);
        (self.strength * retention(age, dynamics.decay_half_life_days, self.potentiated()))
            .clamp(0.0, 1.0)
    }

    /// Reinforce at `now` (a recall that was used, or a re-assertion): decay to `now`, then
    /// restore `reinforcement` of the headroom, `s ← s + r·(1 − s)`, and re-anchor.
    pub fn reinforce(&mut self, dynamics: &Dynamics, now: DateTime<Utc>) {
        let current = self.strength_at(dynamics, now);
        let r = dynamics.reinforcement.clamp(0.0, 1.0);
        self.strength = (current + r * (1.0 - current)).clamp(0.0, 1.0);
        self.anchor_at = now;
    }

    /// Record one use at `now`: reinforce and count it.
    pub fn record_use(&mut self, dynamics: &Dynamics, now: DateTime<Utc>) {
        self.reinforce(dynamics, now);
        self.use_count = self.use_count.saturating_add(1);
        self.last_used_at = Some(now);
    }

    /// Pin or unpin at `now`. Pinning restores full strength; unpinning starts decay from
    /// full strength now, so unpinning never makes a memory weaker than it was a moment ago.
    pub fn set_pinned(&mut self, pinned: bool, now: DateTime<Utc>) {
        self.pinned = pinned;
        self.strength = 1.0;
        self.anchor_at = now;
    }
}

/// When a statement stops being current under its class's expiry rule, if ever.
/// A date property expires at the end of that day plus the grace period.
pub fn expires_at(dynamics: &Dynamics, statement: &ValidStatement) -> Option<DateTime<Utc>> {
    match dynamics.expires_after.as_ref()? {
        Expiry::After { days } => Some(statement.effective_from() + duration_days(*days)),
        Expiry::AtProperty {
            property,
            grace_days,
        } => {
            let deadline = match statement.values(property).first()? {
                Value::Date(date) => {
                    let end_of_day = date.succ_opt()?.and_time(NaiveTime::MIN);
                    Utc.from_utc_datetime(&end_of_day)
                }
                Value::DateTime(at) => at.with_timezone(&Utc),
                _ => return None,
            };
            Some(deadline + duration_days(*grace_days))
        }
    }
}

fn duration_days(days: f64) -> chrono::Duration {
    let millis = (days * DAY_SECONDS * 1000.0).round();
    // Out-of-range values cannot come from a validated ontology (finite, non-negative);
    // saturate instead of overflowing.
    if millis.is_finite() && millis.abs() < i64::MAX as f64 {
        chrono::Duration::milliseconds(millis as i64)
    } else {
        chrono::Duration::MAX
    }
}

/// Importance of a statement in `[IMPORTANCE_FLOOR, 1]`, from four factors (shodh-memory
/// `calculate_importance`, mapped onto ontology classes):
/// - kind of fact (0.05–0.30): decisions highest, then obligations, procedures and
///   preferences, people and projects, tasks and events, episodes and notes;
/// - richness of its text (0.02–0.25) by word count;
/// - entity references (0–0.20): facts that connect entities are more retrievable;
/// - provenance (0–0.20): stated by the user counts fully, otherwise confidence-weighted.
pub fn importance(ontology: &Ontology, statement: &ValidStatement, text: &str) -> f64 {
    let class_score = ontology
        .ancestors(statement.class())
        .iter()
        .find_map(|class| class_importance(&class.id))
        .unwrap_or(0.15);

    let words = text.split_whitespace().count();
    let richness = match words {
        w if w > 50 => 0.25,
        w if w > 20 => 0.15,
        w if w > 5 => 0.08,
        _ => 0.02,
    };

    let entities = statement
        .properties()
        .values()
        .flatten()
        .filter(|v| matches!(v, Value::Entity(_)))
        .count()
        + usize::from(statement.subject().is_some());
    let entity_score = match entities {
        e if e > 10 => 0.20,
        e if e > 5 => 0.15,
        e if e > 2 => 0.10,
        0 => 0.0,
        _ => 0.05,
    };

    let provenance = statement.provenance();
    let provenance_score = match provenance.extractor.kind {
        ExtractorKind::User => 0.20,
        _ => 0.20 * provenance.confidence.clamp(0.0, 1.0),
    };

    (class_score + richness + entity_score + provenance_score).clamp(IMPORTANCE_FLOOR, 1.0)
}

fn class_importance(class: &str) -> Option<f64> {
    Some(match class {
        "Decision" => 0.30,
        "Obligation" | "Procedure" | "Preference" => 0.25,
        "Party" | "Project" | "Concept" | "Contract" => 0.20,
        "Task" | "Event" => 0.15,
        "Episode" | "Note" => 0.10,
        _ => return None,
    })
}

/// A Hebbian link between two statements.
#[derive(Debug, Clone, PartialEq)]
pub struct LinkState {
    /// Weight at `updated_at`, in `[0, 1]`.
    pub weight: f64,
    /// Co-activations so far.
    pub co_activations: u32,
    /// When the weight was last set.
    pub updated_at: DateTime<Utc>,
}

impl LinkState {
    /// The weight at `now`: unused links fade on the same hybrid curve with a 14-day
    /// half-life; links co-activated [`POTENTIATION_USES`] times decay on the potentiated
    /// curve.
    pub fn weight_at(&self, now: DateTime<Utc>) -> f64 {
        let age = days_between(self.updated_at, now);
        (self.weight
            * retention(
                age,
                Some(LINK_HALF_LIFE_DAYS),
                self.co_activations >= POTENTIATION_USES,
            ))
        .clamp(0.0, 1.0)
    }

    /// Strengthen on co-activation at `now` (decay first, then
    /// `w ← w + η·(1 − w)·scale`, with `scale = floor + importance·(1 − floor)`).
    /// Starting from `None` creates the link. The result stays within `[0, 1]`.
    pub fn strengthen(link: Option<&LinkState>, importance: f64, now: DateTime<Utc>) -> Self {
        let (current, count) = match link {
            Some(link) => (link.weight_at(now), link.co_activations),
            None => (0.0, 0),
        };
        let scale = HEBBIAN_IMPORTANCE_FLOOR
            + importance.clamp(0.0, 1.0) * (1.0 - HEBBIAN_IMPORTANCE_FLOOR);
        let weight = (current + HEBBIAN_LEARNING_RATE * (1.0 - current) * scale).clamp(0.0, 1.0);
        Self {
            weight,
            co_activations: count.saturating_add(1),
            updated_at: now,
        }
    }
}

/// The pairs of co-recalled statements whose links are strengthened: every unordered pair
/// of the first [`MAX_CO_RECALLED`] ids (in recall order), each as `(lower, higher)`.
pub fn co_recalled_pairs(ids: &[String]) -> Vec<(String, String)> {
    let mut unique: Vec<&String> = Vec::new();
    for id in ids {
        if !unique.contains(&id) {
            unique.push(id);
        }
        if unique.len() == MAX_CO_RECALLED {
            break;
        }
    }
    let mut pairs = Vec::new();
    for (i, a) in unique.iter().enumerate() {
        for b in &unique[i + 1..] {
            pairs.push(ordered_pair(a, b));
        }
    }
    pairs
}

/// `(a, b)` ordered so the first is the smaller id (links are stored undirected).
pub fn ordered_pair(a: &str, b: &str) -> (String, String) {
    if a <= b {
        (a.to_string(), b.to_string())
    } else {
        (b.to_string(), a.to_string())
    }
}

/// Spreading activation over weighted, undirected links.
///
/// Seeds carry their initial activation. For each hop `d = 1..=hops`, every node whose
/// activation reaches [`SPREADING_THRESHOLD`] passes
/// `a · e^(−λd) · w / √(1 + degree)` to each neighbour (degree normalisation is ACT-R's fan
/// effect, as in shodh-memory). Returns the activation each node *received* (seeds included
/// when reached from another seed), capped at 1. Nodes are visited in id order so the sums
/// are deterministic.
pub fn spread_activation(
    seeds: &[(String, f64)],
    adjacency: &HashMap<String, Vec<(String, f64)>>,
    hops: usize,
) -> BTreeMap<String, f64> {
    let mut received: BTreeMap<String, f64> = BTreeMap::new();
    let mut frontier: BTreeMap<String, f64> = BTreeMap::new();
    for (id, activation) in seeds {
        let entry = frontier.entry(id.clone()).or_insert(0.0);
        *entry = entry.max(activation.clamp(0.0, 1.0));
    }
    for hop in 1..=hops {
        let mut next: BTreeMap<String, f64> = BTreeMap::new();
        let hop_decay = (-SPREADING_DECAY_RATE * hop as f64).exp();
        for (id, activation) in &frontier {
            if *activation < SPREADING_THRESHOLD {
                continue;
            }
            let Some(neighbours) = adjacency.get(id) else {
                continue;
            };
            let fan = 1.0 / (1.0 + neighbours.len() as f64).sqrt();
            for (neighbour, weight) in neighbours {
                let amount = activation * hop_decay * weight.clamp(0.0, 1.0) * fan;
                if amount <= 0.0 {
                    continue;
                }
                *next.entry(neighbour.clone()).or_insert(0.0) += amount;
            }
        }
        for (id, amount) in &next {
            let total = received.entry(id.clone()).or_insert(0.0);
            *total = (*total + amount).min(1.0);
        }
        if next.is_empty() {
            break;
        }
        frontier = next;
    }
    received
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn t0() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap()
    }

    fn dyn_(half_life: Option<f64>, reinforcement: f64) -> Dynamics {
        Dynamics {
            decay_half_life_days: half_life,
            reinforcement,
            expires_after: None,
        }
    }

    #[test]
    fn exponential_leg_honours_the_class_half_life() {
        assert!((retention(30.0, Some(30.0), false) - 0.5).abs() < 1e-12);
        assert!((retention(60.0, Some(30.0), false) - 0.25).abs() < 1e-12);
        assert!((retention(730.0, Some(730.0), false) - 0.5).abs() < 1e-12);
        assert_eq!(retention(0.0, Some(30.0), false), 1.0);
        assert_eq!(retention(-5.0, Some(30.0), false), 1.0);
        assert_eq!(retention(10_000.0, None, false), 1.0);
    }

    #[test]
    fn power_law_tail_is_continuous_and_heavier_than_exponential() {
        let h = 10.0;
        let just_before = retention(CROSSOVER_HALF_LIVES * h - 1e-9, Some(h), false);
        let at = retention(CROSSOVER_HALF_LIVES * h, Some(h), false);
        assert!((just_before - at).abs() < 1e-9);
        assert!((at - 0.125).abs() < 1e-12);
        // Twelve half-lives: pure exponential would leave 1/4096; the tail keeps 1/16.
        let tail = retention(12.0 * h, Some(h), false);
        assert!((tail - 0.125 * 0.5).abs() < 1e-12, "{tail}");
        assert!(tail > 0.5f64.powi(12) * 100.0);
        // Monotone non-increasing.
        let mut last = 1.0;
        for day in 0..400 {
            let r = retention(day as f64, Some(h), false);
            assert!(r <= last + 1e-15, "day {day}");
            last = r;
        }
    }

    #[test]
    fn potentiated_memories_forget_more_slowly() {
        for t in [5.0, 20.0, 50.0, 200.0] {
            assert!(retention(t, Some(10.0), true) > retention(t, Some(10.0), false));
        }
    }

    #[test]
    fn strength_decays_lazily_and_reinforcement_restores_headroom() {
        let d = dyn_(Some(14.0), 0.1);
        let mut s = DynamicsState::fresh(t0(), 0.5);
        assert_eq!(s.strength_at(&d, t0()), 1.0);
        let later = t0() + Duration::days(14);
        assert!((s.strength_at(&d, later) - 0.5).abs() < 1e-9);
        s.record_use(&d, later);
        // 0.5 + 0.1 * 0.5
        assert!((s.strength - 0.55).abs() < 1e-9);
        assert_eq!(s.anchor_at, later);
        assert_eq!(s.use_count, 1);
        assert_eq!(s.last_used_at, Some(later));
        // A no-decay class never weakens.
        let records = dyn_(None, 0.0);
        let fresh = DynamicsState::fresh(t0(), 0.5);
        assert_eq!(
            fresh.strength_at(&records, t0() + Duration::days(9000)),
            1.0
        );
    }

    #[test]
    fn pinned_memories_are_exempt_from_decay() {
        let d = dyn_(Some(14.0), 0.1);
        let mut s = DynamicsState::fresh(t0(), 0.5);
        let later = t0() + Duration::days(28);
        assert!((s.strength_at(&d, later) - 0.25).abs() < 1e-9);
        s.set_pinned(true, later);
        assert_eq!(s.strength_at(&d, later + Duration::days(3650)), 1.0);
        s.set_pinned(false, later + Duration::days(3650));
        assert_eq!(s.strength_at(&d, later + Duration::days(3650)), 1.0);
        assert!(s.strength_at(&d, later + Duration::days(3664)) < 0.51);
    }

    #[test]
    fn hebbian_links_are_bounded_and_saturate() {
        let mut link: Option<LinkState> = None;
        let mut last = 0.0;
        for _ in 0..200 {
            let next = LinkState::strengthen(link.as_ref(), 1.0, t0());
            assert!(next.weight > last && next.weight <= 1.0);
            last = next.weight;
            link = Some(next);
        }
        assert!(last > 0.99);
        let first = LinkState::strengthen(None, 1.0, t0());
        assert!((first.weight - HEBBIAN_LEARNING_RATE).abs() < 1e-12);
        let unimportant = LinkState::strengthen(None, 0.0, t0());
        assert!(
            (unimportant.weight - HEBBIAN_LEARNING_RATE * HEBBIAN_IMPORTANCE_FLOOR).abs() < 1e-12
        );
        // Out-of-range importance cannot push a weight past 1.
        let wild = LinkState::strengthen(link.as_ref(), 50.0, t0());
        assert!(wild.weight <= 1.0);
        // Unused links fade.
        let faded = first.weight_at(t0() + Duration::days(14));
        assert!((faded - first.weight / 2.0).abs() < 1e-9);
    }

    #[test]
    fn co_recalled_pairs_are_bounded_unique_and_ordered() {
        let ids: Vec<String> = (0..20).map(|i| format!("m{i:02}")).collect();
        let pairs = co_recalled_pairs(&ids);
        assert_eq!(pairs.len(), MAX_CO_RECALLED * (MAX_CO_RECALLED - 1) / 2);
        assert!(pairs.iter().all(|(a, b)| a < b));
        let dupes = vec!["b".to_string(), "a".to_string(), "b".to_string()];
        assert_eq!(co_recalled_pairs(&dupes), vec![("a".into(), "b".into())]);
    }

    #[test]
    fn spreading_activation_ranks_close_strong_associations_first() {
        // seed — strong → a — strong → b ; seed — weak → c ; d unconnected
        let mut adj: HashMap<String, Vec<(String, f64)>> = HashMap::new();
        let mut link = |x: &str, y: &str, w: f64| {
            adj.entry(x.into()).or_default().push((y.into(), w));
            adj.entry(y.into()).or_default().push((x.into(), w));
        };
        link("seed", "a", 0.9);
        link("a", "b", 0.9);
        link("seed", "c", 0.1);
        let received = spread_activation(&[("seed".into(), 1.0)], &adj, SPREADING_HOPS);
        let a = received["a"];
        let b = received["b"];
        let c = received["c"];
        assert!(a > b, "one hop beats two: {a} vs {b}");
        assert!(a > c, "strong beats weak: {a} vs {c}");
        assert!(b > 0.0);
        assert!(!received.contains_key("d"));
        assert!(received.values().all(|v| *v <= 1.0));
        // Exact first hop: 1 · e^-0.5 · 0.9 / sqrt(3).
        let expected_a = (-0.5f64).exp() * 0.9 / 3f64.sqrt();
        assert!((a - expected_a).abs() < 1e-12);
        // Deterministic.
        assert_eq!(
            received,
            spread_activation(&[("seed".into(), 1.0)], &adj, SPREADING_HOPS)
        );
        // Hubs are damped: the same edge from a node with many neighbours carries less.
        let mut hub = adj.clone();
        for i in 0..20 {
            hub.entry("seed".into())
                .or_default()
                .push((format!("x{i}"), 0.9));
        }
        let damped = spread_activation(&[("seed".into(), 1.0)], &hub, 1);
        assert!(damped["a"] < a);
    }
}
