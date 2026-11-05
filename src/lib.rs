//! Public compatibility facades grouped by responsibility. See docs/ARCHITECTURE.md.

// Numerical models, measurements and evidence.
pub mod acoustic_fit;
pub mod atmosphere;
pub mod audio;
pub mod audio_priors;
mod cached_pair;
pub mod decision;
pub mod evidence_score;
pub mod fit_diagnostics;
pub mod joint_evidence;
pub mod modality_diagnostics;
pub mod normalization;
pub mod physics;
pub mod single;
pub mod thunder;
pub mod thunder_envelope;
pub mod verification;

// Source acquisition, provider adapters and owned storage.
pub mod acquisition;
pub mod adsblol;
pub mod adsblol_acquisition;
pub mod adsblol_history;
pub mod data_catalog;
pub mod data_size;
pub mod detailed_data;
pub mod fmi_lightning;
pub mod lightning_source;
pub mod receiver_heights;
pub mod terrain;
pub mod track_store;

// Case, review and application workflows.
pub mod casework;
pub mod discovery;
pub mod discovery_groups;
pub mod envelope_workflow;
pub mod event_review;
pub mod gui_bridge;
pub mod joint_workflow;
pub mod map_data;
pub mod media;
pub mod media_tools;
pub mod overlay;
pub mod prediction_overlay;
pub mod receiver_grid;
pub mod regional_search;
pub mod review_media;
pub mod reviewed_search;
pub mod search_setup;
pub mod wide_area_pair;
pub mod workspace;
pub mod workspace_scoring;

// Durable jobs, run history and reports.
pub mod investigation_archive;
pub mod investigation_report;
pub mod jobs;
pub mod regional_history;
pub mod wide_area_journal;

// Internal transport.
mod http_download;
