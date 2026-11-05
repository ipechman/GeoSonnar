//! Manual review workspace. Processing runs only after an explicit action.
mod acoustic_review;
mod atmosphere;
mod fit_comparison;
mod fmi_ui;
mod history_runs;
mod history_ui;
mod inspection;
mod investigation;
mod joint;
mod library;
mod map;
mod map_display;
mod presets;
mod receiver_ui;
mod regional;
#[cfg(test)]
mod tests;
mod tiles;
mod wide_ui;
use super::{
    spectrogram::{self, EditTool, PlotView},
    widgets::*,
};
use eframe::egui::{self, Color32, RichText};
use geosonnar::{
    data_catalog,
    discovery::DiscoverySettings,
    event_review::ReviewSession,
    evidence_score,
    gui_bridge::CaseOverview,
    map_data,
    review_media::{self, AudioPlayback, SpectrogramWindow, VideoFrame},
    thunder,
    workspace::{self, EventKind, EvidenceEvent, ReviewState, ReviewWorkspace, SourceBinding},
};
use presets::{DopplerPreset, ThunderPreset};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    time::{Duration, Instant},
};

mod editing;
mod state;
mod views;
mod workers;

pub use state::{Action, Studio, Tools};
use workers::{PlaybackOwner, Reply, ResultValue, WorkerLane};
