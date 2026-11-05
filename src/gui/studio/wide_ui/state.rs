//! Search settings, source bindings and reply types.
use super::*;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum PlanSection {
    #[default]
    Area,
    Evidence,
    Resources,
    Estimate,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Settings {
    pub(super) event_ids: [String; 2],
    pub(super) cpa_s: [f64; 2],
    pub(super) tones: [[f64; 3]; 2],
    pub(super) speed: [[f64; 2]; 2],
    pub(super) distance: [[f64; 2]; 2],
    pub(super) separation_tolerance_s: f64,
    pub(super) shift: [f64; 3],
    pub(super) utc_start: String,
    pub(super) utc_end: String,
    pub(super) work_limit_enabled: bool,
    pub(super) max_work: u64,
    pub(super) max_output: u64,
    pub(super) max_seconds: u64,
    pub(super) max_index_memory_bytes: u64,
    pub(super) retain: usize,
}

impl Default for Settings {
    fn default() -> Self {
        let budget = WideBudgets::default();
        Self {
            event_ids: Default::default(),
            cpa_s: [0., 0.],
            tones: [[80., 200., 1.]; 2],
            speed: [[40., 330.]; 2],
            distance: [[100., 100_000.]; 2],
            separation_tolerance_s: 30.,
            shift: [-5., 5., 1.],
            utc_start: String::new(),
            utc_end: String::new(),
            work_limit_enabled: false,
            max_work: budget.max_work_units,
            max_output: budget.max_output_bytes,
            max_seconds: budget.max_duration_s,
            max_index_memory_bytes: DEFAULT_INDEX_MEMORY_BYTES,
            retain: 200,
        }
    }
}
#[derive(Clone)]
pub(in crate::gui::studio) struct IndexedSource {
    pub(in crate::gui::studio) binding: TrackStoreBinding,
    pub(in crate::gui::studio) label: String,
    pub(in crate::gui::studio) passes: usize,
    pub(in crate::gui::studio) points: u64,
    pub(in crate::gui::studio) origin: f64,
    pub(in crate::gui::studio) utc_window: [f64; 2],
    pub(in crate::gui::studio) max_gap_s: f64,
}
pub(in crate::gui::studio) struct Prepared {
    pub(super) request: WidePairRequest,
    pub(super) estimate: WideEstimate,
    pub(super) document: ReviewWorkspace,
    pub(super) settings: Settings,
}
#[derive(Clone, Debug, PartialEq)]
pub(in crate::gui::studio) struct IndexReuseScope {
    pub(super) source_catalog_id: String,
    pub(super) acquisition_manifest_sha256: String,
    pub(super) config: geosonnar::adsblol::TraceImportConfig,
    // Exact serialized settings also distinguish signed-zero values, matching
    // the native reuse key rather than f64 PartialEq alone.
    pub(super) canonical_config: String,
}
pub(in crate::gui::studio) struct IndexReuseMatches {
    pub(super) scope: IndexReuseScope,
    pub(super) matches: Vec<data_catalog::IndexedReuseMatch>,
}
pub(in crate::gui::studio) struct ReusedIndexSource {
    pub(super) scope: IndexReuseScope,
    pub(super) source: IndexedSource,
    pub(super) document: Box<ReviewWorkspace>,
}
pub(in crate::gui::studio) enum WideReply {
    Imported(Box<data_catalog::CatalogEntry>),
    IndexMatches(Box<IndexReuseMatches>),
    ReusedSource(Box<ReusedIndexSource>),
    Source(IndexedSource, Box<ReviewWorkspace>),
    Estimated(Box<Prepared>),
    Report(
        Box<WidePairReport>,
        Option<Box<ReviewWorkspace>>,
        Option<WideBudgets>,
    ),
    ReopenedReport(
        Box<WidePairReport>,
        Option<Box<ReviewWorkspace>>,
        Option<WideBudgets>,
    ),
}
pub(in crate::gui::studio) struct WideState {
    pub enabled: bool,
    pub running: bool,
    pub run_id: u64,
    pub cancel: Arc<AtomicBool>,
    pub progress: Option<WideProgress>,
    pub(super) source: Option<IndexedSource>,
    pub(super) settings: Settings,
    pub(super) section: PlanSection,
    pub(super) prepared: Option<Box<Prepared>>,
    pub(super) report: Option<Arc<WidePairReport>>,
    pub(super) report_document: Option<ReviewWorkspace>,
    // Display context from the hash-verified saved request, never active settings.
    pub(super) report_budgets: Option<WideBudgets>,
    pub(super) report_source_missing: bool,
    pub(super) show_results: bool,
    pub(super) sort_column: usize,
    pub(super) sort_reverse: bool,
    pub(super) row_order: Vec<usize>,
    pub(super) started: Option<Instant>,
    pub(super) import_start: String,
    pub(super) import_end: String,
    pub(super) import_padding_km: f64,
    pub(super) import_disk_gib: f64,
    pub(super) index_matches: Option<Box<IndexReuseMatches>>,
}
impl Default for WideState {
    fn default() -> Self {
        Self {
            enabled: false,
            running: false,
            run_id: 0,
            cancel: Arc::new(AtomicBool::new(false)),
            progress: None,
            source: None,
            settings: Settings::default(),
            section: PlanSection::default(),
            prepared: None,
            report: None,
            report_document: None,
            report_budgets: None,
            report_source_missing: false,
            show_results: false,
            sort_column: 0,
            sort_reverse: false,
            row_order: vec![],
            started: None,
            import_start: String::new(),
            import_end: String::new(),
            import_padding_km: 100.,
            import_disk_gib: 8.,
            index_matches: None,
        }
    }
}
