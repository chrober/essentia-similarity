# Essentia Similarity

Host-independent Rust research engine for General song similarity from offline Essentia-derived audio features. **This code does not implement or validate a production General Metric, Personal Metric, Lyrion plugin or Guidance SPI v2 service.**

## Independent Rust workspace

| Crate | Responsibilities |
| --- | --- |
| `essentia-similarity-core` | Pure versioned cosine / per-vector centered cosine geometry; strict finite/bounded dimension checks; aggregate-only comparison of a host's baseline ranks with research scores. **No database or player dependency.** |
| `essentia-feature-store` | Strict read-only SQLite v3 `feature_models` and `feature_tracks` decoding, exact opaque `feature_tracks.track_file` identities, fingerprint/class and Essentia reference-validation checks. **No player-specific identity resolution.** |
| `essentia-similarity-shadow` | Standalone local research CLI; compares complete already-admitted candidate lists (2–50), emits **aggregate-only** ranking differences, never changes a queue or playlist. |

The code was extracted from [`chrober/bliss-mood-poc`](https://github.com/chrober/bliss-mood-poc), research branch `feat/general-pairwise-similarity-research` (30 October 2026). The scanner and WP1/WP2/WP3 quality research remain there. Extraction removed the relative Rust `#[path]` source include and player-specific fields from the core. The feature store consumes the **actual scanner SQLite v3 schema** and does not import or reanalyse audio. The original research implementation had passing Linux/Windows CI; this repo has its own independent CI.

```sh
cargo test --workspace --all-targets
cargo fmt --all -- --check
cargo run -p essentia-similarity-shadow -- --request /trusted/local/request.json
```

## Local shadow request (not an SPI protocol)

The trusted caller supplies `schema=essentia-similarity-sidecar-shadow-v1`, `mode=shadow_only`, a trusted absolute `feature_database` path, exact opaque `anchor_track_file`, `model_id` (`genre` or `moodtheme`), versioned `metric` (`affine_cosine_v1` or `affine_centered_cosine_v1`), `expected_model_key`, `experimental_preview_opt_in=false`, and 2–50 already host-admitted `candidates`, each with unique `candidate_id`, exact `track_file`, `baseline_rank` (0-based), and `admitted_by_host=true`.

The host must supply canonical **exact sidecar keys**. The store does **not** infer an original Bliss file key from LMS urlmd5 or fuzzy artist/title. A missing source file, fingerprint mismatch, invalid/degenerate vector, nonvalidated Essentia reference, label-order mismatch or incomplete candidate pool triggers a neutral **whole-query** fallback. Explicit unvalidated opt-in is **shadow-only**, never production. The output contains no artist, title, track path, raw scores, candidate IDs or personal judgments.

Current heads are generic SQLite v3 **Genre87 and Mood56**. The **Embedding1280** representation, a selected or calibrated General Metric, independent listening-quality uplift, actual sidecar quality and true per-user Personal Metric are outstanding. A fixed `(1+cosine)/2` geometric score is not evidence of cross-model calibration. No inference or source-audio reading at playlist runtime. Third-party Essentia, ONNX model and audio licensing/distribution remain separate gates; this repository does not bundle external weights or audio.

## Other repositories

- [`chrober/bliss-mood-poc`](https://github.com/chrober/bliss-mood-poc): audio feature scanner and independent research/evaluation. Owns the original offline storage schema.
- [`chrober/bliss-guidance-essentia`](https://github.com/chrober/bliss-guidance-essentia): future host-specific **native SPI v2 adapter**, not present here.
- [`chrober/lms-guidance-essentia`](https://github.com/chrober/lms-guidance-essentia): future installable Lyrion plugin, not present here.

A future Lyrion/Bliss host owns exact external identity mapping, candidate admission and hard constraints; Guidance can only advise over that already qualified shortlist. Direct feature-first/hybrid candidate discovery requires a separate host-owned admission API, not an SPI shortcut.
