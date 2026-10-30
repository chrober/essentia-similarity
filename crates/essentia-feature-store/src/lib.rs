//! Exact opaque-key, read-only Mood/Genre feature lookup for SQLite v3.
//! Independent of any music player, playback and runtime audio inference.
use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use serde::Serialize;
use std::{collections::{BTreeMap, BTreeSet}, path::Path};

const MAX_TRACKS: usize = 256;

#[derive(Debug, Serialize)]
pub struct TrackFeatureLookup {
    pub track_file: String,
    pub models: BTreeMap<String, ModelLookup>,
}

#[derive(Debug, Serialize)]
pub struct ModelLookup {
    /// missing / invalid / unvalidated / experimental / validated
    pub status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preprocessing_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aggregation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub validated_against_essentia: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub labels: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scores_f32: Option<Vec<f32>>,
}

impl ModelLookup {
    fn status(status: &'static str) -> Self {
        Self {
            status,
            model_key: None,
            preprocessing_version: None,
            aggregation: None,
            validated_against_essentia: None,
            labels: None,
            scores_f32: None,
        }
    }
}

/// The caller owns candidate admission; this accessor never scans the library
/// or invents track identities from artist/title strings.
pub fn inspect(
    database: &Path,
    track_files: &[String],
    requested_models: &str,
    include_experimental: bool,
) -> Result<Vec<TrackFeatureLookup>> {
    if track_files.is_empty() || track_files.len() > MAX_TRACKS {
        bail!("Inspect between 1 and {MAX_TRACKS} exact sidecar track keys per request");
    }
    if track_files.iter().any(|key| key.is_empty() || key.contains('\0')) {
        bail!("Invalid exact sidecar track key");
    }
    let models = requested_models.split(',')
        .map(str::trim).collect::<Vec<_>>();
    let distinct = models.iter().copied().collect::<BTreeSet<_>>();
    if models.is_empty() || models.len() > 2 || distinct.len() != models.len()
        || models.iter().any(|id| !matches!(*id, "moodtheme" | "genre"))
    {
        bail!("--models must specify unique moodtheme and/or genre");
    }

    // SQLite OPEN_READ_ONLY deliberately rejects a missing file rather than
    // accidentally creating an empty sidecar.
    let conn = Connection::open_with_flags(database, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("Cannot open feature sidecar read-only: {}", database.display()))?;
    let version: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version != 3 { bail!("Feature lookup requires schema v3; found v{version}"); }
    inspect_connection(&conn, track_files, &models, include_experimental)
}

struct StoredHeadRow {
    capability: String,
    model_key: String,
    preprocessing_version: String,
    aggregation: String,
    classes_json: String,
    validated: i64,
    bytes: Vec<u8>,
}

fn inspect_connection(
    conn: &Connection,
    track_files: &[String],
    models: &[&str],
    include_experimental: bool,
) -> Result<Vec<TrackFeatureLookup>> {
    let mut statement = conn.prepare(
        "SELECT t.capability, t.model_key, m.preprocessing_version,
                m.aggregation, m.classes_json, m.validated_against_essentia,
                t.scores_f32_le
         FROM feature_tracks t
         JOIN feature_models m ON m.model_key = t.model_key
         WHERE t.track_file=?1 AND t.model_id=?2"
    )?;
    let mut result = Vec::with_capacity(track_files.len());
    for key in track_files {
        let mut heads = BTreeMap::new();
        for model_id in models {
            let found = statement.query_row(params![key, model_id], |row| {
                Ok(StoredHeadRow {
                    capability: row.get(0)?,
                    model_key: row.get(1)?,
                    preprocessing_version: row.get(2)?,
                    aggregation: row.get(3)?,
                    classes_json: row.get(4)?,
                    validated: row.get(5)?,
                    bytes: row.get(6)?,
                })
            }).optional()?;
            let head = match found {
                None => ModelLookup::status("missing"),
                Some(stored) => decode_head(model_id, stored, include_experimental),
            };
            heads.insert((*model_id).to_owned(), head);
        }
        result.push(TrackFeatureLookup { track_file: key.clone(), models: heads });
    }
    Ok(result)
}

fn decode_head(id: &str, stored: StoredHeadRow, include_experimental: bool) -> ModelLookup {
    let StoredHeadRow {
        capability, model_key, preprocessing_version, aggregation,
        classes_json, validated, bytes,
    } = stored;
    let (expected_capability, expected_count) = match id {
        "moodtheme" => ("mood_theme", 56),
        "genre" => ("genre", 87),
        _ => return ModelLookup::status("invalid"),
    };
    let labels = serde_json::from_str::<Vec<String>>(&classes_json).ok();
    let valid_labels = labels.as_ref().is_some_and(|labels| {
        labels.len() == expected_count
            && labels.iter().all(|v| !v.is_empty())
            && labels.iter().collect::<BTreeSet<_>>().len() == labels.len()
    });
    if capability != expected_capability || model_key.is_empty()
        || preprocessing_version.is_empty() || aggregation.is_empty()
        || !matches!(validated, 0 | 1) || !valid_labels
        || bytes.len() != expected_count * 4
    {
        return ModelLookup::status("invalid");
    }
    let mut scores = Vec::with_capacity(expected_count);
    for word in bytes.as_chunks::<4>().0 {
        let value = f32::from_le_bytes(*word);
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return ModelLookup::status("invalid");
        }
        scores.push(value);
    }
    let validated = validated == 1;
    let visible = validated || include_experimental;
    ModelLookup {
        status: if validated { "validated" } else if include_experimental {
            "experimental"
        } else { "unvalidated" },
        model_key: Some(model_key),
        preprocessing_version: Some(preprocessing_version),
        aggregation: Some(aggregation),
        validated_against_essentia: Some(validated),
        labels: if visible { labels } else { None },
        scores_f32: if visible { Some(scores) } else { None },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
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
             );"
        ).unwrap();
        for (id, capability, n) in [("moodtheme", "mood_theme", 56), ("genre", "genre", 87)] {
            let fingerprint = format!("model-{id}");
            let labels = (0..n).map(|x| format!("label{x:03}")).collect::<Vec<_>>();
            let bytes: Vec<u8> = (0..n).flat_map(|_| 0.5_f32.to_le_bytes()).collect();
            conn.execute(
                "INSERT INTO feature_models VALUES(?1,?2,?3,?4,0)",
                params![fingerprint, "preprocess-v1", "mean-patch", serde_json::to_string(&labels).unwrap()]
            ).unwrap();
            conn.execute(
                "INSERT INTO feature_tracks VALUES(?1,?2,?3,?4,?5)",
                params!["Artist/Album/track.flac", id, capability, fingerprint, bytes]
            ).unwrap();
        }
        conn
    }

    #[test]
    fn exact_keys_and_unvalidated_default_are_safe() {
        let conn = fixture();
        let keys = vec!["Artist/Album/track.flac".to_owned(), "Artist/Album/Track.flac".to_owned()];
        let result = inspect_connection(&conn, &keys, &["moodtheme", "genre"], false).unwrap();
        assert_eq!(result[0].models["moodtheme"].status, "unvalidated");
        assert!(result[0].models["genre"].scores_f32.is_none());
        assert_eq!(result[1].models["genre"].status, "missing");
        let experimental = inspect_connection(&conn, &keys, &["genre"], true).unwrap();
        assert_eq!(experimental[0].models["genre"].status, "experimental");
        assert_eq!(experimental[0].models["genre"].scores_f32.as_ref().unwrap().len(), 87);
    }

    #[test]
    fn malformed_one_head_is_neutral_without_affecting_another() {
        let conn = fixture();
        conn.execute("UPDATE feature_tracks SET scores_f32_le=X'00000000' WHERE model_id='genre'", []).unwrap();
        let keys = vec!["Artist/Album/track.flac".to_owned()];
        let result = inspect_connection(&conn, &keys, &["moodtheme", "genre"], true).unwrap();
        assert_eq!(result[0].models["genre"].status, "invalid");
        assert!(result[0].models["genre"].scores_f32.is_none());
        assert_eq!(result[0].models["moodtheme"].status, "experimental");
    }

    #[test]
    fn validated_catalog_entry_exposes_scores_without_override() {
        let conn = fixture();
        conn.execute("UPDATE feature_models SET validated_against_essentia=1 WHERE model_key='model-genre'", []).unwrap();
        let keys = vec!["Artist/Album/track.flac".to_owned()];
        let result = inspect_connection(&conn, &keys, &["genre"], false).unwrap();
        assert_eq!(result[0].models["genre"].status, "validated");
        assert_eq!(result[0].models["genre"].scores_f32.as_ref().unwrap().len(), 87);
    }
}
