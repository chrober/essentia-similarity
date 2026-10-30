//! Read-only local Essentia feature SQLite v3 shadow comparison.
//! Opaque feature keys and admitted candidate ranks must come from the caller.
//! NOT an SPI v2 provider, playlist writer or calibrated General Metric.
use essentia_feature_store as feature_lookup;

use essentia_similarity_core::{
    compare_full_candidate_pool, geometric_similarity, Metric, RankedCandidate, ShadowRankSummary,
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::Path;
use std::process::ExitCode;

const SCHEMA: &str = "essentia-similarity-sidecar-shadow-v1";
const RESULT_SCHEMA: &str = "essentia-similarity-sidecar-shadow-result-v1";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Candidate {
    candidate_id: String,
    /// Exact host-confirmed feature_tracks.track_file key, not an artist/title guess.
    track_file: String,
    baseline_rank: usize,
    admitted_by_host: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    schema: String,
    mode: String,
    feature_database: String,
    anchor_track_file: String,
    model_id: String,
    metric: String,
    expected_model_key: String,
    experimental_preview_opt_in: bool,
    candidates: Vec<Candidate>,
}

#[derive(Serialize)]
struct Report {
    schema: &'static str,
    mode: &'static str,
    status: &'static str,
    candidate_count: usize,
    shadow_reranking_eligible: bool,
    neutral_reasons: Vec<&'static str>,
    changed_positions: usize,
    top5_overlap: Option<usize>,
    max_absolute_rank_shift: Option<usize>,
    experimental_preview: bool,
    scored_with_historically_selected_or_production_general_metric: bool,
    personal_metric_active: bool,
    spi_v2_provider_implemented: bool,
    output_contains_track_identifiers_or_scores: bool,
    playlist_or_queue_mutated: bool,
}

fn neutral(n: usize, optin: bool, mut reasons: Vec<&'static str>) -> Report {
    reasons.sort_unstable();
    reasons.dedup();
    Report {
        schema: RESULT_SCHEMA,
        mode: "shadow_only",
        status: "NEUTRAL_FULL_QUERY_BASELINE_FALLBACK",
        candidate_count: n,
        shadow_reranking_eligible: false,
        neutral_reasons: reasons,
        changed_positions: 0,
        top5_overlap: None,
        max_absolute_rank_shift: None,
        experimental_preview: optin,
        scored_with_historically_selected_or_production_general_metric: false,
        personal_metric_active: false,
        spi_v2_provider_implemented: false,
        output_contains_track_identifiers_or_scores: false,
        playlist_or_queue_mutated: false,
    }
}

fn preview(n: usize, optin: bool, aggregate: ShadowRankSummary) -> Report {
    Report {
        schema: RESULT_SCHEMA,
        mode: "shadow_only",
        status: "RESEARCH_SHADOW_RANK_AGGREGATE_ONLY",
        candidate_count: n,
        shadow_reranking_eligible: true,
        neutral_reasons: Vec::new(),
        changed_positions: aggregate.changed_positions,
        top5_overlap: Some(aggregate.top5_overlap),
        max_absolute_rank_shift: Some(aggregate.max_absolute_rank_shift),
        experimental_preview: optin,
        scored_with_historically_selected_or_production_general_metric: false,
        personal_metric_active: false,
        spi_v2_provider_implemented: false,
        output_contains_track_identifiers_or_scores: false,
        playlist_or_queue_mutated: false,
    }
}

/// Errors are ONLY for invalid/insecure invocation envelopes and candidate
/// admissions; data/feature availability errors instead produce a neutral
/// whole-query report, never a partially reranked pool.
fn run(request: &Request) -> Result<Report, &'static str> {
    if request.schema != SCHEMA || request.mode != "shadow_only" {
        return Err("only_versioned_shadow_mode_is_supported");
    }
    let metric = Metric::parse(&request.metric)?;
    if !matches!(request.model_id.as_str(), "genre" | "moodtheme") {
        return Err("unsupported_sidecar_head");
    }
    if request.feature_database.is_empty()
        || request.anchor_track_file.is_empty()
        || request.anchor_track_file.contains('\0')
        || request.expected_model_key.is_empty()
        || !(2..=50).contains(&request.candidates.len())
    {
        return Err("invalid_bounded_trusted_feature_request");
    }
    let mut ids = HashSet::new();
    let mut tracks = HashSet::new();
    let mut ranks = HashSet::new();
    for candidate in &request.candidates {
        if candidate.candidate_id.is_empty()
            || candidate.track_file.is_empty()
            || candidate.track_file.contains('\0')
            || !candidate.admitted_by_host
            || candidate.baseline_rank >= request.candidates.len()
            || !ids.insert(candidate.candidate_id.as_str())
            || !tracks.insert(candidate.track_file.as_str())
            || !ranks.insert(candidate.baseline_rank)
        {
            return Err("untrusted_or_duplicate_candidate_pool");
        }
    }
    let n = request.candidates.len();
    let keys = std::iter::once(request.anchor_track_file.clone())
        .chain(request.candidates.iter().map(|x| x.track_file.clone()))
        .collect::<Vec<_>>();

    let fetched = match feature_lookup::inspect(
        Path::new(&request.feature_database),
        &keys,
        &request.model_id,
        request.experimental_preview_opt_in,
    ) {
        Ok(rows) if rows.len() == n + 1 => rows,
        _ => {
            return Ok(neutral(
                n,
                request.experimental_preview_opt_in,
                vec!["read_only_sidecar_unavailable_or_schema_mismatch"],
            ))
        }
    };

    let mut reasons = Vec::new();
    let mut arrays = Vec::with_capacity(n + 1);
    let mut reference_labels: Option<Vec<String>> = None;
    for row in &fetched {
        let Some(head) = row.models.get(&request.model_id) else {
            reasons.push("requested_head_missing");
            continue;
        };
        match head.status {
            "missing" => {
                reasons.push("feature_row_missing");
                continue;
            }
            "invalid" => {
                reasons.push("invalid_feature_row");
                continue;
            }
            "unvalidated" => {
                reasons.push("essentia_reference_validation_missing");
                continue;
            }
            "experimental" if !request.experimental_preview_opt_in => {
                reasons.push("essentia_reference_validation_missing");
                continue;
            }
            "validated" | "experimental" => {}
            _ => {
                reasons.push("unexpected_feature_status");
                continue;
            }
        }
        if head.model_key.as_deref() != Some(request.expected_model_key.as_str()) {
            reasons.push("feature_model_fingerprint_mismatch");
            continue;
        }
        let (Some(labels), Some(scores)) = (&head.labels, &head.scores_f32) else {
            reasons.push("feature_vector_not_available");
            continue;
        };
        if let Some(expected) = &reference_labels {
            if labels != expected {
                reasons.push("class_label_order_mismatch");
                continue;
            }
        } else {
            reference_labels = Some(labels.clone());
        }
        arrays.push(scores.as_slice());
    }
    if !reasons.is_empty() || arrays.len() != n + 1 {
        if reasons.is_empty() {
            reasons.push("incomplete_comparable_feature_batch");
        }
        return Ok(neutral(n, request.experimental_preview_opt_in, reasons));
    }

    let anchor = arrays[0];
    let mut ranked = Vec::with_capacity(n);
    for (c, vector) in request.candidates.iter().zip(arrays.iter().skip(1)) {
        let score = match geometric_similarity(anchor, vector, metric) {
            Ok(score) => score,
            Err(_) => {
                return Ok(neutral(
                    n,
                    request.experimental_preview_opt_in,
                    vec!["degenerate_or_invalid_geometry"],
                ))
            }
        };
        ranked.push(RankedCandidate {
            baseline_rank: c.baseline_rank,
            shadow_score: score,
        });
    }
    match compare_full_candidate_pool(&ranked) {
        Ok(summary) => Ok(preview(n, request.experimental_preview_opt_in, summary)),
        Err(_) => Ok(neutral(
            n,
            request.experimental_preview_opt_in,
            vec!["invalid_complete_shadow_ranking"],
        )),
    }
}

fn main() -> ExitCode {
    let args = std::env::args().collect::<Vec<_>>();
    if args.len() != 3 || args[1] != "--request" {
        eprintln!("Expected --request <trusted-local-json-file>");
        return ExitCode::from(2);
    }
    // Read trusted local file only. Never log paths, candidate IDs or scores.
    let bytes = match std::fs::read(&args[2]) {
        Ok(b) if b.len() <= 100_000 => b,
        _ => {
            eprintln!("Shadow request unreadable or exceeds bound");
            return ExitCode::from(2);
        }
    };
    let request = match serde_json::from_slice::<Request>(&bytes) {
        Ok(req) => req,
        Err(_) => {
            eprintln!("Invalid research request envelope");
            return ExitCode::from(2);
        }
    };
    let report = match run(&request) {
        Ok(r) => r,
        Err(code) => {
            // Stable non-sensitive reason; never serialize original request.
            eprintln!("Research-only request refused: {code}");
            return ExitCode::from(2);
        }
    };
    match serde_json::to_string(&report) {
        Ok(serialized) => {
            println!("{serialized}");
            ExitCode::SUCCESS
        }
        Err(_) => ExitCode::from(2),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::{params, Connection};
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TempSidecar(std::path::PathBuf);
    impl Drop for TempSidecar {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    fn fixture(validated: i64) -> TempSidecar {
        let micros = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let file = std::env::temp_dir().join(format!(
            "essentia-shadow-fictional-{}-{micros}.sqlite",
            std::process::id()
        ));
        let conn = Connection::open(&file).unwrap();
        // Compatible with the scanner's real read-only SELECT and v3 version.
        conn.execute_batch(
            "PRAGMA user_version=3;
             CREATE TABLE feature_models (
                model_key TEXT PRIMARY KEY, preprocessing_version TEXT NOT NULL,
                aggregation TEXT NOT NULL, classes_json TEXT NOT NULL,
                validated_against_essentia INTEGER NOT NULL
             );
             CREATE TABLE feature_tracks (
                track_file TEXT NOT NULL, model_id TEXT NOT NULL,
                capability TEXT NOT NULL, model_key TEXT NOT NULL,
                scores_f32_le BLOB NOT NULL,
                PRIMARY KEY(track_file, model_id)
             );",
        )
        .unwrap();
        let labels = (0..87).map(|x| format!("label{x:03}")).collect::<Vec<_>>();
        conn.execute(
            "INSERT INTO feature_models VALUES (?1,?2,?3,?4,?5)",
            params![
                "fake-pinned-model-key",
                "fixed-preprocess-v1",
                "patch-mean",
                serde_json::to_string(&labels).unwrap(),
                validated
            ],
        )
        .unwrap();
        for (track, shift) in [
            ("artist/anchor.flac", 0),
            ("artist/one.flac", 7),
            ("artist/two.flac", 21),
            ("artist/three.flac", 42),
        ] {
            let values = (0..87)
                .map(|j| 0.05 + (((j + shift) * 31) % 83) as f32 / 100.0)
                .collect::<Vec<_>>();
            let bytes: Vec<u8> = values.iter().flat_map(|x| x.to_le_bytes()).collect();
            conn.execute(
                "INSERT INTO feature_tracks VALUES (?1,?2,?3,?4,?5)",
                params![track, "genre", "genre", "fake-pinned-model-key", bytes],
            )
            .unwrap();
        }
        drop(conn);
        TempSidecar(file)
    }

    fn request(sidecar: &TempSidecar) -> Request {
        Request {
            schema: SCHEMA.into(),
            mode: "shadow_only".into(),
            feature_database: sidecar.0.to_string_lossy().into_owned(),
            anchor_track_file: "artist/anchor.flac".into(),
            model_id: "genre".into(),
            metric: "affine_centered_cosine_v1".into(),
            expected_model_key: "fake-pinned-model-key".into(),
            experimental_preview_opt_in: false,
            candidates: ["artist/one.flac", "artist/two.flac", "artist/three.flac"]
                .into_iter()
                .enumerate()
                .map(|(i, track_file)| Candidate {
                    candidate_id: format!("anonymous-synthetic-candidate-{i}"),
                    track_file: track_file.to_string(),
                    baseline_rank: i,
                    admitted_by_host: true,
                })
                .collect(),
        }
    }

    #[test]
    fn reads_real_sqlite_schema_v3_read_only_and_emits_only_aggregates() {
        let fixture = fixture(1);
        let req = request(&fixture);
        let report = run(&req).unwrap();
        assert_eq!(report.status, "RESEARCH_SHADOW_RANK_AGGREGATE_ONLY");
        assert_eq!(report.candidate_count, 3);
        assert!(report.shadow_reranking_eligible);
        let json = serde_json::to_string(&report).unwrap();
        assert!(!json.contains("anonymous-synthetic"));
        assert!(!json.contains("artist/"));
        assert!(!json.contains("fake-pinned-model-key"));
        assert!(!report.playlist_or_queue_mutated);
        assert!(!report.spi_v2_provider_implemented);
        // Original sidecar remains queryable and unchanged.
        let conn = Connection::open(&fixture.0).unwrap();
        let n: i64 = conn
            .query_row("SELECT count(*) FROM feature_tracks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 4);
    }

    #[test]
    fn wrong_exact_key_or_fingerprint_falls_back_for_entire_pool() {
        let f = fixture(1);
        let mut req = request(&f);
        req.candidates[1].track_file = "artist/Two.flac".into();
        let res = run(&req).unwrap();
        assert_eq!(res.status, "NEUTRAL_FULL_QUERY_BASELINE_FALLBACK");
        assert!(res.neutral_reasons.contains(&"feature_row_missing"));
        assert_eq!(res.changed_positions, 0);
        req.candidates[1].track_file = "artist/two.flac".into();
        req.expected_model_key = "wrong-fingerprint".into();
        let res = run(&req).unwrap();
        assert!(res
            .neutral_reasons
            .contains(&"feature_model_fingerprint_mismatch"));
    }

    #[test]
    fn unvalidated_is_disabled_unless_explicit_shadow_preview_optin() {
        let f = fixture(0);
        let mut req = request(&f);
        let res = run(&req).unwrap();
        assert!(res
            .neutral_reasons
            .contains(&"essentia_reference_validation_missing"));
        req.experimental_preview_opt_in = true;
        let res = run(&req).unwrap();
        assert!(res.shadow_reranking_eligible);
        assert!(res.experimental_preview);
        assert!(!res.playlist_or_queue_mutated);
    }

    #[test]
    fn malformed_source_database_yields_neutral_read_only_fallback() {
        let f = fixture(1);
        let mut req = request(&f);
        req.feature_database.push_str(".does-not-exist");
        let res = run(&req).unwrap();
        assert!(res
            .neutral_reasons
            .contains(&"read_only_sidecar_unavailable_or_schema_mismatch"));
        assert_eq!(res.top5_overlap, None);
    }

    #[test]
    fn rejects_external_admissions_and_non_shadow_requests() {
        let f = fixture(1);
        let mut req = request(&f);
        req.candidates[0].admitted_by_host = false;
        assert_eq!(
            run(&req).err(),
            Some("untrusted_or_duplicate_candidate_pool")
        );
        let mut req = request(&f);
        req.mode = "queue_apply".into();
        assert_eq!(
            run(&req).err(),
            Some("only_versioned_shadow_mode_is_supported")
        );
        let mut req = request(&f);
        req.candidates[0].baseline_rank = 1;
        assert_eq!(
            run(&req).err(),
            Some("untrusted_or_duplicate_candidate_pool")
        );
        let mut req = request(&f);
        req.model_id = "embedding".into();
        assert_eq!(run(&req).err(), Some("unsupported_sidecar_head"));
    }
}
