//! Standalone mathematical core for exploratory GENERAL similarity.
//!
//! No audio, player dependencies, SQLite, model training or personal data.
//! These predeclared geometric candidates have NOT passed perceptual-quality
//! or cross-model calibration gates and are NOT production mixing scores.
//! The host owns candidate admission, exact file-key resolution, routing and
//! hard repeat/genre constraints. This crate never modifies a playlist.

use std::collections::HashSet;

pub const CONTRACT_VERSION: &str = "essentia-similarity-geometry-v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Metric {
    Cosine,
    CenteredCosine,
}

impl Metric {
    pub fn parse(value: &str) -> Result<Self, &'static str> {
        match value {
            "affine_cosine_v1" => Ok(Self::Cosine),
            "affine_centered_cosine_v1" => Ok(Self::CenteredCosine),
            _ => Err("unsupported_versioned_metric"),
        }
    }
}

/// Scoring uses the same per-vector mean-centering semantics as
/// scripts/evaluate_similarity.py, not a corpus mean or a learned transform.
/// Return (clipped cosine + 1) / 2, a bounded *geometric* rank proxy; do NOT
/// infer that this mapping establishes perceptual calibration or permits
/// raw blending with the host baseline, other heads, or personal scores.
pub fn geometric_similarity(a: &[f32], b: &[f32], metric: Metric) -> Result<f64, &'static str> {
    if a.len() != b.len() || a.is_empty() {
        return Err("dimension_mismatch");
    }
    if !a
        .iter()
        .chain(b)
        .all(|&x| x.is_finite() && (0.0..=1.0).contains(&x))
    {
        return Err("invalid_sigmoid_vector");
    }
    fn mean_variance(xs: &[f32]) -> (f64, f64) {
        let n = xs.len() as f64;
        let mean = xs.iter().map(|x| f64::from(*x)).sum::<f64>() / n;
        let variance = xs
            .iter()
            .map(|x| (f64::from(*x) - mean).powi(2))
            .sum::<f64>()
            / n;
        (mean, variance)
    }
    let (ma, va) = mean_variance(a);
    let (mb, vb) = mean_variance(b);
    // Same deliberately conservative 1e-10 degenerate-vector rule as WP1.
    if va <= 1e-10 || vb <= 1e-10 {
        return Err("degenerate_classification_vector");
    }

    let (ma, mb) = match metric {
        Metric::Cosine => (0.0, 0.0),
        Metric::CenteredCosine => (ma, mb),
    };
    let mut dot = 0.0_f64;
    let mut na = 0.0_f64;
    let mut nb = 0.0_f64;
    for (&x, &y) in a.iter().zip(b) {
        let x = f64::from(x) - ma;
        let y = f64::from(y) - mb;
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    if na <= 1e-12 || nb <= 1e-12 {
        return Err("zero_similarity_norm");
    }
    let cosine = dot / (na * nb).sqrt();
    if !cosine.is_finite() {
        return Err("invalid_similarity_result");
    }
    Ok((cosine.clamp(-1.0, 1.0) + 1.0) / 2.0)
}

#[derive(Clone, Copy, Debug)]
pub struct RankedCandidate {
    pub baseline_rank: usize,
    /// A geometric score, NOT a calibrated or chosen General Metric.
    pub shadow_score: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShadowRankSummary {
    pub candidate_count: usize,
    pub changed_positions: usize,
    pub top5_overlap: usize,
    pub max_absolute_rank_shift: usize,
}

/// Return only summary counts, never candidate IDs or a proposed real route.
/// Fail entirely on incomplete/invalid rank or score vectors.
/// Equal scores preserve original the host baseline rank; no stochastic reranking.
pub fn compare_full_candidate_pool(
    candidates: &[RankedCandidate],
) -> Result<ShadowRankSummary, &'static str> {
    if candidates.len() < 2 || candidates.len() > 50 {
        return Err("invalid_bounded_candidate_pool");
    }
    if candidates.iter().any(|x| {
        !x.shadow_score.is_finite()
            || !(0.0..=1.0).contains(&x.shadow_score)
            || x.baseline_rank >= candidates.len()
    }) {
        return Err("invalid_complete_candidate_score_or_rank");
    }
    let mut seen = HashSet::new();
    for item in candidates {
        if !seen.insert(item.baseline_rank) {
            return Err("duplicate_baseline_rank");
        }
    }
    let mut ranked = candidates.to_vec();
    ranked.sort_by(|a, b| a.baseline_rank.cmp(&b.baseline_rank));
    let mut reordered = ranked.clone();
    reordered.sort_by(|a, b| {
        b.shadow_score
            .total_cmp(&a.shadow_score)
            .then(a.baseline_rank.cmp(&b.baseline_rank))
    });

    let mut changed = 0;
    let mut max_shift = 0;
    for (index, c) in reordered.iter().enumerate() {
        if index != c.baseline_rank {
            changed += 1;
        }
        max_shift = max_shift.max(index.abs_diff(c.baseline_rank));
    }
    let k = candidates.len().min(5);
    let top5_overlap = reordered[..k].iter().filter(|c| c.baseline_rank < k).count();
    Ok(ShadowRankSummary {
        candidate_count: candidates.len(),
        changed_positions: changed,
        top5_overlap,
        max_absolute_rank_shift: max_shift,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versioned_metric_names_are_explicit() {
        assert_eq!(Metric::parse("affine_cosine_v1").unwrap(), Metric::Cosine);
        assert_eq!(
            Metric::parse("affine_centered_cosine_v1").unwrap(),
            Metric::CenteredCosine
        );
        assert!(Metric::parse("cosine").is_err());
        assert!(Metric::parse("mood_centered_prod").is_err());
    }

    #[test]
    fn cosine_and_centered_cosine_match_reference_geometry() {
        let x = [0.9, 0.3, 0.2, 0.1];
        let y = [0.1, 0.2, 0.3, 0.9];
        for metric in [Metric::Cosine, Metric::CenteredCosine] {
            let same = geometric_similarity(&x, &x, metric).unwrap();
            assert!((same - 1.0).abs() < 1e-12);
            let ab = geometric_similarity(&x, &y, metric).unwrap();
            let ba = geometric_similarity(&y, &x, metric).unwrap();
            assert!((ab - ba).abs() < 1e-12);
            assert!((0.0..=1.0).contains(&ab));
        }
        let a = [0.1, 0.4, 0.7, 0.9];
        let b = [0.9, 0.6, 0.3, 0.1];
        assert!(geometric_similarity(&a, &b, Metric::CenteredCosine).unwrap() < 0.01);
    }

    #[test]
    fn dimensional_nonfinite_uniform_and_range_checks_fail_closed() {
        let x = [0.1, 0.8, 0.2, 0.3];
        for invalid in [
            vec![0.1],
            vec![0.5; 4],
            vec![0.1, f32::NAN, 0.2, 0.3],
            vec![0.1, f32::INFINITY, 0.2, 0.3],
            vec![0.1, -0.2, 0.2, 0.3],
            vec![0.1, 1.2, 0.2, 0.3],
        ] {
            for metric in [Metric::Cosine, Metric::CenteredCosine] {
                assert!(geometric_similarity(&x, &invalid, metric).is_err());
            }
        }
    }

    #[test]
    fn reorders_shadow_only_without_identity_or_raw_score_output() {
        let scores = [0.65, 0.95, 0.30, 0.85, 0.45, 0.90];
        let source: Vec<_> = scores
            .into_iter()
            .enumerate()
            .map(|(rank, s)| RankedCandidate {
                baseline_rank: rank,
                shadow_score: s,
            })
            .collect();
        let baseline = source.iter().map(|x| x.baseline_rank).collect::<Vec<_>>();
        let report = compare_full_candidate_pool(&source).unwrap();
        assert_eq!(
            source.iter().map(|x| x.baseline_rank).collect::<Vec<_>>(),
            baseline
        );
        assert_eq!(
            report,
            ShadowRankSummary {
                candidate_count: 6,
                changed_positions: 5,
                top5_overlap: 4,
                max_absolute_rank_shift: 4,
            }
        );
    }

    #[test]
    fn ties_preserve_original_rank_and_partial_queries_are_rejected() {
        let tied = (0..6)
            .rev()
            .map(|i| RankedCandidate {
                baseline_rank: i,
                shadow_score: 0.5,
            })
            .collect::<Vec<_>>();
        let r = compare_full_candidate_pool(&tied).unwrap();
        assert_eq!(r.changed_positions, 0);
        assert_eq!(r.top5_overlap, 5);
        let mut dup = tied.clone();
        dup[1].baseline_rank = 0;
        assert!(compare_full_candidate_pool(&dup).is_err());
        let mut nan = tied.clone();
        nan[2].shadow_score = f64::NAN;
        assert!(compare_full_candidate_pool(&nan).is_err());
        assert!(compare_full_candidate_pool(&[]).is_err());
    }
}
