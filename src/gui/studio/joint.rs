//! Explicit shared-cell frequency/envelope review. No source or event is selected here.
use super::*;
use geosonnar::{
    envelope_workflow::{self, MeasurementConfig},
    investigation_archive::{JointSnapshot, ReportKind},
    joint_evidence::{
        self, DopplerState, EnvelopeState, JointEstimate, JointProgress, JointRequest, JointStage,
        JointStatus,
    },
    thunder_envelope::{EnvelopeLimits, EnvelopeModel, EnvelopeReceiver, EnvelopeSearchPlan},
    workspace::{EnvelopeReview, EnvelopeSelection},
};

#[derive(Clone, Debug, PartialEq)]
pub(super) struct JointOptions {
    catalog_start_utc: String,
    catalog_end_utc: String,
    minimum_distance_m: f64,
    maximum_distance_m: f64,
    kernel_decay_s: f64,
    kernel_duration_s: f64,
    max_cells: usize,
    max_envelope_work: u64,
}
impl Default for JointOptions {
    fn default() -> Self {
        Self {
            catalog_start_utc: String::new(),
            catalog_end_utc: String::new(),
            minimum_distance_m: 100.0,
            maximum_distance_m: 50_000.0,
            kernel_decay_s: 5.0,
            kernel_duration_s: 24.0,
            max_cells: 25_000,
            max_envelope_work: 200_000_000,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum JointView {
    Doppler,
    Envelope,
    Combined,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum JointColumn {
    Rank,
    Latitude,
    Longitude,
    Clock,
    Doppler,
    Envelope,
    Status,
}

pub(super) struct JointState {
    pub enabled: bool,
    pub running: bool,
    pub run_id: u64,
    pub cancel: Arc<AtomicBool>,
    measurement_run_id: Option<u64>,
    pub show_results: bool,
    options: JointOptions,
    prepared: Option<PreparedJoint>,
    progress: Option<JointProgress>,
    pub snapshot: Option<Arc<JointSnapshot>>,
    request_only: Option<Arc<JointRequest>>,
    pub(super) report_document: Option<ReviewWorkspace>,
    view: JointView,
    sort: JointColumn,
    descending: bool,
    sort_dirty: bool,
    rows: Vec<usize>,
    selected: Option<usize>,
    summary: Vec<String>,
}
impl Default for JointState {
    fn default() -> Self {
        Self {
            enabled: false,
            running: false,
            run_id: 0,
            cancel: Arc::new(AtomicBool::new(false)),
            measurement_run_id: None,
            show_results: true,
            options: JointOptions::default(),
            prepared: None,
            progress: None,
            snapshot: None,
            request_only: None,
            report_document: None,
            view: JointView::Combined,
            sort: JointColumn::Rank,
            descending: false,
            sort_dirty: true,
            rows: Vec::new(),
            selected: None,
            summary: Vec::new(),
        }
    }
}
struct PreparedJoint {
    request: Arc<JointRequest>,
    estimate: JointEstimate,
    options: JointOptions,
    source_revision: u64,
}
pub(super) struct MeasurementReply {
    pub run_id: u64,
    pub original: ReviewWorkspace,
    pub result: Result<geosonnar::thunder_envelope::ObservedEnvelope, String>,
}
pub(super) struct EstimateReply {
    pub run_id: u64,
    pub request: Arc<JointRequest>,
    pub options: JointOptions,
    pub source_revision: u64,
    pub result: Result<JointEstimate, String>,
}

fn doppler_score(cell: &joint_evidence::JointCell) -> Option<f64> {
    match cell.doppler {
        DopplerState::Available { score, .. } => Some(score),
        _ => None,
    }
}
fn envelope_score(cell: &joint_evidence::JointCell) -> Option<f64> {
    match cell.envelope {
        EnvelopeState::Available {
            cosine_similarity, ..
        } => Some(cosine_similarity),
        _ => None,
    }
}
fn rank(cell: &joint_evidence::JointCell, view: JointView) -> Option<f64> {
    cell.ranks.as_ref().map(|r| match view {
        JointView::Doppler => r.doppler,
        JointView::Envelope => r.envelope,
        JointView::Combined => r.combined,
    })
}
fn cell_status(cell: &joint_evidence::JointCell) -> &'static str {
    if matches!(cell.doppler, DopplerState::NotStarted)
        || matches!(cell.envelope, EnvelopeState::NotStarted)
    {
        "Not completed"
    } else if doppler_score(cell).is_some() && envelope_score(cell).is_some() {
        "Both scores available"
    } else {
        "Unavailable evidence"
    }
}
fn compare_optional(a: Option<f64>, b: Option<f64>, descending: bool) -> std::cmp::Ordering {
    match (a, b) {
        (Some(a), Some(b)) => {
            if descending {
                b.total_cmp(&a)
            } else {
                a.total_cmp(&b)
            }
        }
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        _ => std::cmp::Ordering::Equal,
    }
}
fn sorted_cells(
    report: &joint_evidence::JointReport,
    view: JointView,
    column: JointColumn,
    descending: bool,
) -> Vec<usize> {
    let value = |cell: &joint_evidence::JointCell| match column {
        JointColumn::Rank => rank(cell, view),
        JointColumn::Latitude => Some(report.receivers[cell.receiver_index].location.latitude_deg),
        JointColumn::Longitude => {
            Some(report.receivers[cell.receiver_index].location.longitude_deg)
        }
        JointColumn::Clock => Some(cell.clock_offset_s),
        JointColumn::Doppler => doppler_score(cell),
        JointColumn::Envelope => envelope_score(cell),
        JointColumn::Status => None,
    };
    let mut rows: Vec<_> = (0..report.cells.len()).collect();
    rows.sort_by(|&a, &b| {
        let left = &report.cells[a];
        let right = &report.cells[b];
        let order = if column == JointColumn::Status {
            let order = cell_status(left).cmp(cell_status(right));
            if descending { order.reverse() } else { order }
        } else {
            compare_optional(value(left), value(right), descending)
        };
        order
            .then_with(|| left.receiver_index.cmp(&right.receiver_index))
            .then_with(|| left.clock_index.cmp(&right.clock_index))
    });
    rows
}
fn number(value: Option<f64>, digits: usize) -> String {
    value.map_or_else(|| "—".into(), |v| format!("{v:.digits$}"))
}
fn utc_input(value: f64) -> String {
    let label = regional::utc_label(value);
    if value.fract() == 0.0 && label.len() >= 19 {
        format!("{}Z", &label[..19])
    } else {
        label.replace(" UTC", "Z")
    }
}
fn options_for(request: &JointRequest) -> JointOptions {
    let plan = &request.envelope_plan;
    JointOptions {
        catalog_start_utc: utc_input(plan.catalog_utc_interval[0]),
        catalog_end_utc: utc_input(plan.catalog_utc_interval[1]),
        minimum_distance_m: plan.model.minimum_distance_m,
        maximum_distance_m: plan.model.maximum_distance_m,
        kernel_decay_s: plan.model.kernel_decay_s,
        kernel_duration_s: plan.model.kernel_duration_s,
        max_cells: request.limits.max_cells,
        max_envelope_work: request.limits.max_envelope_work_units,
    }
}
fn state_reason(cell: &joint_evidence::JointCell) -> String {
    let mut reasons = Vec::new();
    match &cell.doppler {
        DopplerState::Unavailable { reason } => reasons.push(format!("Doppler: {reason}")),
        DopplerState::NotStarted => reasons.push("Doppler was not completed".into()),
        _ => {}
    }
    match &cell.envelope {
        EnvelopeState::Unavailable { reason } => reasons.push(format!("Envelope: {reason}")),
        EnvelopeState::NotStarted => reasons.push("Envelope was not completed".into()),
        _ => {}
    }
    reasons.join(" · ")
}
fn summary(report: &joint_evidence::JointReport) -> Vec<String> {
    let d = report
        .cells
        .iter()
        .filter(|c| doppler_score(c).is_some())
        .count();
    let e = report
        .cells
        .iter()
        .filter(|c| envelope_score(c).is_some())
        .count();
    let both = report
        .cells
        .iter()
        .filter(|c| doppler_score(c).is_some() && envelope_score(c).is_some())
        .count();
    let mut lines = vec![format!(
        "{} receivers × {} clocks = {} declared cells · Doppler {d} · envelope {e} · both {both}",
        report.receivers.len(),
        report.clock_offsets_s.len(),
        report.cells.len()
    )];
    let best_d = report
        .cells
        .iter()
        .filter_map(doppler_score)
        .min_by(f64::total_cmp);
    let best_e = report
        .cells
        .iter()
        .filter_map(envelope_score)
        .max_by(f64::total_cmp);
    if let (Some(ds), Some(es)) = (best_d, best_e) {
        let d_rank = report
            .cells
            .iter()
            .filter_map(|c| rank(c, JointView::Doppler))
            .min_by(f64::total_cmp);
        let e_rank = report
            .cells
            .iter()
            .filter_map(|c| rank(c, JointView::Envelope))
            .min_by(f64::total_cmp);
        // Prefer the core's complete tolerance groups, including its documented
        // transitive ties. In incomplete reports there are no full-domain ranks.
        let ds: Vec<_> = report
            .cells
            .iter()
            .filter(|c| {
                if let Some(best) = d_rank {
                    rank(c, JointView::Doppler) == Some(best)
                } else {
                    doppler_score(c)
                        .is_some_and(|v| (v - ds).abs() <= joint_evidence::DOPPLER_TOLERANCE)
                }
            })
            .collect();
        let es: Vec<_> = report
            .cells
            .iter()
            .filter(|c| {
                if let Some(best) = e_rank {
                    rank(c, JointView::Envelope) == Some(best)
                } else {
                    envelope_score(c)
                        .is_some_and(|v| (v - es).abs() <= joint_evidence::ENVELOPE_TOLERANCE)
                }
            })
            .collect();
        let es_keys: std::collections::HashSet<_> = es
            .iter()
            .map(|c| (c.receiver_index, c.clock_index))
            .collect();
        let shared = ds
            .iter()
            .filter(|c| es_keys.contains(&(c.receiver_index, c.clock_index)))
            .count();
        lines.push(format!("Leading Doppler cells: {} · leading envelope cells: {} · shared location AND clock: {shared}",ds.len(),es.len()));
        if !report.ranking_available {
            lines.push("These leading scores describe available work only; they are not complete-domain ranks.".into());
        }
        if let (Some(d), Some(e)) = (ds.first(), es.first()) {
            let a = &report.receivers[d.receiver_index].location;
            let b = &report.receivers[e.receiver_index].location;
            let p = a.latitude_deg.to_radians();
            let q = b.latitude_deg.to_radians();
            let h = ((q - p) / 2.0).sin().powi(2)
                + p.cos()
                    * q.cos()
                    * ((b.longitude_deg - a.longitude_deg).to_radians() / 2.0)
                        .sin()
                        .powi(2);
            let distance = 2.0 * 6_371_008.8 * h.clamp(0.0, 1.0).sqrt().asin();
            lines.push(format!("Representative leading cells: {:.1} km apart · clock difference {:.3} s. Ties may have other separations.",distance/1000.0,(d.clock_offset_s-e.clock_offset_s).abs()));
        }
    }
    lines
}

impl Studio {
    fn spawn_joint<F>(&mut self, ctx: &egui::Context, label: &str, task: F)
    where
        F: FnOnce() -> Result<ResultValue, String> + Send + 'static,
    {
        if self.busy() || self.exiting {
            return;
        }
        self.busy = Some(label.into());
        self.notice = None;
        self.spawn_lane(ctx, WorkerLane::JointTask(self.joint.run_id), task);
    }
    pub(super) fn joint_audio_ui(&mut self, ui: &mut egui::Ui) {
        if self.busy() && self.joint.measurement_run_id == Some(self.joint.run_id) {
            ui.horizontal_wrapped(|ui| {
                ui.label("Measuring the reviewed envelope…");
                if ui.button("Cancel envelope measurement").clicked() {
                    self.joint.cancel.store(true, Ordering::Relaxed);
                }
                if self.joint.cancel.load(Ordering::Relaxed) {
                    ui.label("Cancellation requested");
                }
            });
        }
        let Some(doc) = self.document.as_ref() else {
            return;
        };
        let measured = doc
            .envelope_review
            .as_ref()
            .is_some_and(|r| r.observation.is_some());
        let title = if measured {
            "Thunder envelope — measured interval"
        } else {
            "Thunder envelope — optional comparison interval"
        };
        egui::CollapsingHeader::new(title).id_salt("joint_audio_selection").show(ui, |ui| {
            muted(ui,"Select a stable interval of the original audio, review it, then measure. Zooming the spectrogram does not change this selection. No thunder event or source is approved here.");
            let busy=self.busy();
            let mut measure=false;
            ui.add_enabled_ui(!busy,|ui| {
                if ui.button("Use displayed time interval").clicked() {
                    let doc=self.document.as_mut().unwrap();
                    doc.envelope_review=Some(EnvelopeReview { selection: EnvelopeSelection {
                        start_s:self.view.time_min_s.max(0.0), end_s:self.view.time_max_s.min(doc.duration_s),
                        fft_size:4096,hop_s:1.0,band_hz:[20.0,250.0],reviewed_selection_note:String::new(),
                    },observation:None });
                    self.joint.prepared=None;
                }
                let doc=self.document.as_mut().unwrap();
                let Some(review)=doc.envelope_review.as_mut() else { return; };
                let before=review.selection.clone();
                ui.horizontal_wrapped(|ui| {
                    ui.label("Start");ui.add(egui::DragValue::new(&mut review.selection.start_s).range(0.0..=doc.duration_s).suffix(" s"));
                    ui.label("End");ui.add(egui::DragValue::new(&mut review.selection.end_s).range(0.0..=doc.duration_s).suffix(" s"));
                    ui.label(format!("Channel {}",doc.channel+1));
                });
                ui.label("Review note (required)");
                accessible_text(ui,"joint_selection_note",&mut review.selection.reviewed_selection_note,"Describe the interval and why its envelope is useful",true,ui.available_width());
                if self.detailed_controls {
                    ui.horizontal_wrapped(|ui| {
                        ui.label("FFT");egui::ComboBox::from_id_salt("joint_fft").selected_text(review.selection.fft_size.to_string()).show_ui(ui,|ui| {
                            for size in [1024,2048,4096,8192,16384] {ui.selectable_value(&mut review.selection.fft_size,size,size.to_string());}
                        });
                        ui.label("Hop");ui.add(egui::DragValue::new(&mut review.selection.hop_s).range(0.05..=5.0).speed(0.05).suffix(" s"));
                        ui.label("Band");ui.add(egui::DragValue::new(&mut review.selection.band_hz[0]).range(20.0..=249.0).suffix(" Hz"));
                        ui.add(egui::DragValue::new(&mut review.selection.band_hz[1]).range(21.0..=250.0).suffix(" Hz"));
                    });
                } else { muted(ui,format!("FFT {} · every {:.3} s · {:.1}–{:.1} Hz. More settings are available in All controls.",review.selection.fft_size,review.selection.hop_s,review.selection.band_hz[0],review.selection.band_hz[1])); }
                if review.selection!=before {review.observation=None;self.joint.prepared=None;}
                let valid=review.selection.validate(doc.duration_s).and_then(|_| if review.selection.reviewed_selection_note.trim().is_empty() {Err("Add a review note before measuring this interval".into())} else {Ok(())});
                if let Err(error)=&valid {ui.colored_label(tone(ui,AMBER),error);}
                if let Some(observed)=&review.observation { ui.label(format!("{} measured RMS samples · actual band {:.3}–{:.3} Hz",observed.envelope.samples.len(),observed.envelope.band_low_hz,observed.envelope.band_high_hz)); }
                measure=ui.add_enabled(valid.is_ok(),egui::Button::new("Measure reviewed envelope")).clicked();
            });
            if measure {self.measure_joint_envelope(ui.ctx());}
        });
    }
    fn measure_joint_envelope(&mut self, ctx: &egui::Context) {
        if self.busy() {
            return;
        }
        let (Some(case), Some(original)) = (self.case.as_ref(), self.document.clone()) else {
            return;
        };
        let Some(review) = original.envelope_review.as_ref() else {
            return;
        };
        if let Err(error) = review.selection.validate(original.duration_s) {
            self.notice = Some((true, error));
            return;
        }
        if review.selection.reviewed_selection_note.trim().is_empty() {
            self.notice = Some((
                true,
                "Review the envelope interval before measuring it.".into(),
            ));
            return;
        }
        let config = MeasurementConfig {
            schema_version: 1,
            recording: case.recording_path.clone(),
            channel: original.channel,
            start_s: review.selection.start_s,
            end_s: review.selection.end_s,
            fft_size: review.selection.fft_size,
            hop_s: review.selection.hop_s,
            band_hz: review.selection.band_hz,
            reviewed_selection_note: review.selection.reviewed_selection_note.clone(),
            ffmpeg: Some(self.tools.ffmpeg.clone()),
            ffprobe: Some(self.tools.ffprobe.clone()),
        };
        let manifest = case.manifest.clone();
        let base = manifest
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .to_path_buf();
        self.joint.run_id = self.joint.run_id.wrapping_add(1);
        let run_id = self.joint.run_id;
        self.joint.measurement_run_id = Some(run_id);
        self.joint.cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.joint.cancel.clone();
        self.spawn_joint(
            ctx,
            "Measuring the reviewed thunder envelope…",
            move || {
                let result = workspace::verify(&manifest, &original)
                    .and_then(|_| envelope_workflow::measure_observation(&config, &base, &cancel))
                    .and_then(|value| {
                        workspace::verify(&manifest, &original)?;
                        Ok(value)
                    });
                Ok(ResultValue::JointMeasured(Box::new(MeasurementReply {
                    run_id,
                    original,
                    result,
                })))
            },
        );
    }
    pub(super) fn accept_joint_measurement(&mut self, reply: MeasurementReply) {
        if reply.run_id == self.joint.run_id {
            self.joint.measurement_run_id = None;
        }
        if reply.run_id != self.joint.run_id || self.document.as_ref() != Some(&reply.original) {
            self.notice=Some((true,"Review or selection changed during measurement. Your edits were kept; measure the current interval explicitly.".into()));
            return;
        }
        if self.joint.cancel.load(Ordering::Relaxed) {
            self.notice = Some((
                false,
                "Envelope measurement cancelled; the previous review was kept.".into(),
            ));
            return;
        }
        match reply.result {
            Ok(observation) => {
                let mut next = reply.original.clone();
                let Some(review) = next.envelope_review.as_mut() else {
                    return;
                };
                review.observation = Some(observation);
                if let Err(error) = next.validate() {
                    self.notice = Some((true, error));
                    return;
                }
                self.document = Some(next);
                self.checkpoint(reply.original);
                self.joint.prepared = None;
                self.notice=Some((false,"Envelope measured and saved in the review draft. No event or source was selected; continue to Search when ready.".into()));
            }
            Err(error) => self.notice = Some((true, error)),
        }
    }
    pub(super) fn joint_method_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.label("Method");
            ui.selectable_value(&mut self.joint.enabled, false, "Reviewed events");
            ui.selectable_value(
                &mut self.joint.enabled,
                true,
                "Doppler + envelope (experimental)",
            );
        });
        if self.joint.enabled {
            muted(
                ui,
                "Compares Doppler frequency with the full reviewed thunder envelope at the same location AND clock. Picked thunder arrivals are not an extra vote. Switching methods does not alter evidence.",
            );
        }
    }
    pub(super) fn joint_run_ui(&mut self, ui: &mut egui::Ui) {
        ui.strong("Review shared-cell comparison");
        muted(
            ui,
            "Experimental ordering, not location confidence. Models can disagree; combined ranking always has a best row when all cells are available.",
        );
        let Some(doc) = self.document.as_ref() else {
            return;
        };
        let blocked = joint_block_reason(doc);
        if let Some(reason) = &blocked {
            ui.colored_label(tone(ui, AMBER), reason);
        }
        if let Some(review) = &doc.envelope_review {
            ui.label(format!(
                "Reviewed audio {:.3}–{:.3} s · channel {} · {}",
                review.selection.start_s,
                review.selection.end_s,
                doc.channel + 1,
                if review.observation.is_some() {
                    "measured"
                } else {
                    "not measured"
                }
            ));
        } else {
            muted(
                ui,
                "Choose and measure an interval under Audio review → Thunder envelope.",
            );
        }
        ui.strong("Lightning catalog query coverage — UTC");
        muted(
            ui,
            "Enter the interval actually requested from the provider. First/last recorded strikes do not establish query coverage. This is a declared search support interval, not a claim that all lightning was detected.",
        );
        accessible_text(
            ui,
            "joint_catalog_start",
            &mut self.joint.options.catalog_start_utc,
            "Start: YYYY-MM-DD HH:MM:SSZ",
            false,
            ui.available_width(),
        );
        accessible_text(
            ui,
            "joint_catalog_end",
            &mut self.joint.options.catalog_end_utc,
            "End, exclusive: YYYY-MM-DD HH:MM:SSZ",
            false,
            ui.available_width(),
        );
        if let Some(catalog) = &self.lightning {
            ui.label(format!(
                "Loaded lightning: {} rows · {}",
                catalog.strikes.len(),
                catalog.provenance.provider
            ));
        } else {
            ui.colored_label(
                tone(ui, AMBER),
                "Load an explicitly reviewed lightning catalog in Data.",
            );
        }
        self.joint_model_ui(ui);
        if ui
            .add_enabled(
                blocked.is_none() && self.lightning.is_some(),
                egui::Button::new("Estimate shared-cell comparison"),
            )
            .clicked()
        {
            self.estimate_joint(ui.ctx());
        }
        let current = self.joint_prepared_current();
        if let Some(prepared) = &self.joint.prepared {
            let e = &prepared.estimate;
            ui.label(format!(
                "{} receivers × {} clocks = {} cells · {} Doppler + {} envelope runs",
                e.receivers, e.clocks, e.cells, e.doppler_runs, e.envelope_runs
            ));
            ui.label(format!(
                "Working memory bound {} · saved snapshot bound {}",
                geosonnar::data_size::format_gib(e.working_memory_bound_bytes),
                geosonnar::data_size::format_gib(e.serialized_snapshot_bound_bytes)
            ));
            if self.detailed_controls {
                ui.label(format!("{} Doppler evaluations · {} residual evaluations · {} envelope work units (including all retained/direct checks)",e.doppler_evaluations,e.doppler_residual_evaluations,e.envelope_work_units));
            }
            if !current {
                ui.colored_label(
                    tone(ui, AMBER),
                    "Review, source data or settings changed. Estimate again before running.",
                );
            }
        }
        if ui
            .add_enabled(
                current && !self.has_unsaved_results(),
                egui::Button::new("Run reviewed shared-cell comparison"),
            )
            .clicked()
        {
            self.run_joint(ui.ctx());
        }
        if self.has_unsaved_results() {
            ui.colored_label(tone(ui,AMBER),"Save or explicitly discard the unsaved result in Results before starting another run.");
        }
        muted(
            ui,
            "The complete declared grid is retained, including missing and unfinished cells. A durable start record must be written before computation. No prediction is applied automatically.",
        );
    }
    fn joint_model_ui(&mut self, ui: &mut egui::Ui) {
        let options = &mut self.joint.options;
        let doc = self.document.as_mut().unwrap();
        egui::CollapsingHeader::new("Envelope model and work limits").default_open(self.detailed_controls).show(ui,|ui| {
            muted(ui,"Equal source energies; inverse-square attenuation; finite exponential energy response. Provider intensity/current is not acoustic strength. The resulting cosine is not a probability.");
            ui.label(format!("Sound speed: {:.2} m/s (from the current review)",doc.sound_speed_mps));
            egui::Grid::new("joint_model").num_columns(2).show(ui,|ui| {
                for (label,value) in [("Minimum distance (m)",&mut options.minimum_distance_m),("Maximum distance (m)",&mut options.maximum_distance_m),("Energy decay (s)",&mut options.kernel_decay_s),("Response duration (s)",&mut options.kernel_duration_s)] {
                    ui.label(label);ui.add(egui::DragValue::new(value).speed(0.5));ui.end_row();
                }
                let mut fallback=doc.strike_fallback_height_m.is_some();
                if ui.checkbox(&mut fallback,"Assume height for unknown lightning").changed() {doc.strike_fallback_height_m=if fallback {Some(0.0)} else {None};}
                if let Some(height)=doc.strike_fallback_height_m.as_mut() {ui.add(egui::DragValue::new(height).range(-1000.0..=100_000.0).suffix(" m ellipsoid"));} else {ui.label("No fallback assumption");}
                ui.end_row();
                ui.label("Maximum cells");ui.add(egui::DragValue::new(&mut options.max_cells).range(1..=joint_evidence::MAX_CELLS));ui.end_row();
                ui.label("Envelope work limit");ui.add(egui::DragValue::new(&mut options.max_envelope_work).range(1..=geosonnar::thunder_envelope::MAX_WORK_UNITS).speed(100_000));ui.end_row();
            });
        });
    }
    fn joint_prepared_current(&self) -> bool {
        self.joint.prepared.as_ref().is_some_and(|p| {
            self.document.as_ref() == Some(&p.request.document)
                && self.joint.options == p.options
                && self.loaded_data_revision == p.source_revision
        })
    }
    fn estimate_joint(&mut self, ctx: &egui::Context) {
        if self.busy() {
            return;
        }
        let Some(document) = self.document.clone() else {
            return;
        };
        if let Some(reason) = joint_block_reason(&document) {
            self.notice = Some((true, reason));
            return;
        }
        let request = (|| {
            self.cached_bindings_match(&document)?;
            let map = self.merged_map()?;
            let lightning = self
                .lightning
                .clone()
                .ok_or("Load lightning data before estimating")?;
            let observation = document
                .envelope_review
                .as_ref()
                .and_then(|r| r.observation.clone())
                .ok_or("Measure the reviewed envelope first")?;
            let regional_plan = document
                .regional_plan
                .clone()
                .ok_or("Set up a regional plan first")?;
            let options = &self.joint.options;
            let envelope_plan = EnvelopeSearchPlan {
                schema_version: 1,
                catalog_sha256: lightning.input_sha256.clone(),
                catalog_utc_interval: [
                    workspace::parse_utc(&options.catalog_start_utc)?,
                    workspace::parse_utc(&options.catalog_end_utc)?,
                ],
                receivers: Vec::new(),
                nominal_recording_start_utc_s: workspace::parse_utc(&document.recording_start_utc)?,
                min_clock_offset_s: regional_plan.clock.min_offset_s,
                max_clock_offset_s: regional_plan.clock.max_offset_s,
                model: EnvelopeModel {
                    sound_speed_mps: document.sound_speed_mps,
                    fallback_strike_ellipsoid_height_m: document.strike_fallback_height_m,
                    minimum_distance_m: options.minimum_distance_m,
                    maximum_distance_m: options.maximum_distance_m,
                    kernel_decay_s: options.kernel_decay_s,
                    kernel_duration_s: options.kernel_duration_s,
                },
                source_policy: document.mix.lightning_source_policy.clone(),
                keep_best: geosonnar::thunder_envelope::MAX_KEEP_BEST,
                limits: EnvelopeLimits {
                    max_work_units: options.max_envelope_work,
                    max_memory_bytes: joint_evidence::MAX_MEMORY_BYTES,
                },
            };
            let limits = joint_evidence::JointLimits {
                max_cells: options.max_cells,
                max_envelope_work_units: options.max_envelope_work,
                ..Default::default()
            };
            Ok::<_, String>(JointRequest {
                schema_version: 1,
                document,
                map,
                lightning,
                observation,
                regional_plan,
                envelope_plan,
                limits,
            })
        })();
        let mut request = match request {
            Ok(r) => r,
            Err(error) => {
                self.notice = Some((true, error));
                return;
            }
        };
        let manifest = self.case.as_ref().unwrap().manifest.clone();
        let options = self.joint.options.clone();
        let source_revision = self.loaded_data_revision;
        self.joint.run_id = self.joint.run_id.wrapping_add(1);
        let run_id = self.joint.run_id;
        self.joint.prepared = None;
        self.spawn_joint(
            ctx,
            "Checking the complete shared-cell grid and work limits…",
            move || {
                let result = (|| {
                    workspace::verify(&manifest, &request.document)?;
                    // Terrain sampling and file verification belong to this worker,
                    // never the frame that draws or activates the Estimate button.
                    request.envelope_plan.receivers =
                        geosonnar::receiver_heights::grid(&request.document)?
                            .complete_observers()?
                            .into_iter()
                            .enumerate()
                            .map(|(i, location)| EnvelopeReceiver {
                                id: format!("receiver-{i:06}"),
                                location,
                            })
                            .collect();
                    let estimate = joint_evidence::estimate(&request)?;
                    workspace::verify(&manifest, &request.document)?;
                    Ok(estimate)
                })();
                Ok(ResultValue::JointEstimated(Box::new(EstimateReply {
                    run_id,
                    request: Arc::new(request),
                    options,
                    source_revision,
                    result,
                })))
            },
        );
    }
    pub(super) fn accept_joint_estimate(&mut self, reply: EstimateReply) {
        if reply.run_id != self.joint.run_id
            || self.document.as_ref() != Some(&reply.request.document)
            || self.joint.options != reply.options
            || self.loaded_data_revision != reply.source_revision
        {
            self.notice=Some((true,"Review, settings or loaded data changed while estimating. Estimate the current comparison again.".into()));
            return;
        }
        match reply.result {
            Ok(estimate) => {
                self.joint.prepared = Some(PreparedJoint {
                    request: reply.request,
                    estimate,
                    options: reply.options,
                    source_revision: reply.source_revision,
                });
                self.notice=Some((false,"Full shared-cell comparison estimated. Review the bounds and run explicitly when ready.".into()));
            }
            Err(error) => self.notice = Some((true, error)),
        }
    }
    fn run_joint(&mut self, ctx: &egui::Context) {
        if self.busy() || !self.joint_prepared_current() || self.has_unsaved_results() {
            return;
        }
        let request = self.joint.prepared.as_ref().unwrap().request.clone();
        let manifest = self.case.as_ref().unwrap().manifest.clone();
        self.joint.run_id = self.joint.run_id.wrapping_add(1);
        let run_id = self.joint.run_id;
        self.joint.cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.joint.cancel.clone();
        self.joint.running = true;
        self.joint.progress = None;
        let sender = self.sender.clone();
        let generation = self.generation;
        let repaint = ctx.clone();
        self.spawn_joint(
            ctx,
            "Comparing Doppler and envelope at every shared cell…",
            move || {
                let started = (|| {
                    workspace::verify(&manifest, &request.document)?;
                    let inputs = joint_evidence::inputs_sha256(&request)?;
                    let handle = geosonnar::regional_history::begin_joint(
                        &manifest,
                        &request.document,
                        &request,
                        &inputs,
                    )?;
                    Ok::<_, String>((inputs, handle))
                })();
                let (inputs_sha256, handle) = match started {
                    Ok(value) => value,
                    Err(error) => {
                        return Ok(ResultValue::JointStopped(
                            run_id,
                            format!("Comparison was not started: {error}"),
                        ));
                    }
                };
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let mut last = Instant::now() - Duration::from_secs(1);
                    let report = joint_evidence::run(&request, &cancel, |progress| {
                        if last.elapsed() >= Duration::from_millis(120)
                            || progress.stage == JointStage::Finished
                        {
                            let _ = sender.send(Reply {
                                generation,
                                lane: WorkerLane::SearchProgress,
                                result: Ok(ResultValue::JointProgress(run_id, progress.clone())),
                            });
                            last = Instant::now();
                            repaint.request_repaint();
                        }
                    })?;
                    workspace::verify(&manifest, &request.document)?;
                    Ok::<_, String>(ReportKind::JointEnvelope(Box::new(JointSnapshot {
                        request: (*request).clone(),
                        report,
                    })))
                }))
                .unwrap_or_else(|_| Err("Shared-cell comparison stopped unexpectedly".into()));
                let cancelled = cancel.load(Ordering::Relaxed);
                let persistence = geosonnar::regional_history::finish(handle, &outcome, cancelled);
                Ok(ResultValue::RunRecorded(Box::new(
                    history_runs::RunCompletion {
                        original_manifest: manifest.clone(),
                        document: request.document.clone(),
                        inputs_sha256,
                        outcome,
                        cancelled,
                        persistence,
                        joint_request: Some((*request).clone()),
                    },
                )))
            },
        );
    }
    pub(super) fn accept_joint_stopped(&mut self, run_id: u64, error: String) {
        if run_id == self.joint.run_id {
            self.joint.running = false;
            self.notice = Some((true, error));
        }
    }
    pub(super) fn accept_joint_progress(&mut self, run_id: u64, progress: JointProgress) {
        if run_id == self.joint.run_id && self.joint.running {
            self.joint.progress = Some(progress);
        }
    }
    pub(super) fn joint_progress_ui(&mut self, ui: &mut egui::Ui) {
        if !self.joint.running {
            return;
        }
        ui.horizontal_wrapped(|ui| {
            if let Some(p) = &self.joint.progress {
                let done = p.completed_doppler_cells + p.completed_envelope_cells;
                let total = p.total_cells.saturating_mul(2).max(1);
                ui.add(egui::ProgressBar::new(done as f32 / total as f32).desired_width(160.0));
                ui.label(format!(
                    "{} · Doppler {} / {} · envelope {} / {} cells",
                    match p.stage {
                        JointStage::Doppler => "Doppler",
                        JointStage::Envelope => "Envelope",
                        JointStage::Ranking => "Ranking",
                        JointStage::Finished => "Saving",
                    },
                    p.completed_doppler_cells,
                    p.total_cells,
                    p.completed_envelope_cells,
                    p.total_cells
                ));
            } else {
                ui.label("Preparing durable shared-cell run…");
            }
            if ui.button("Cancel shared-cell comparison").clicked() {
                self.joint.cancel.store(true, Ordering::Relaxed);
            }
            if self.joint.cancel.load(Ordering::Relaxed) {
                ui.label("Cancellation requested; uncomputed cells remain explicit.");
            }
        });
    }
    pub(super) fn accept_joint_report(
        &mut self,
        snapshot: JointSnapshot,
        document: ReviewWorkspace,
    ) {
        self.joint.running = false;
        self.joint.summary = summary(&snapshot.report);
        self.joint.options = options_for(&snapshot.request);
        self.joint.request_only = None;
        self.joint.snapshot = Some(Arc::new(snapshot));
        self.joint.report_document = Some(document);
        self.joint.prepared = None;
        self.joint.show_results = true;
        self.joint.sort_dirty = true;
        self.joint.selected = None;
        self.regional.report = None;
        self.regional.comparison = None;
        self.score = None;
        self.score_document = None;
        self.pending_action = Some(Action::ReviewResults);
    }
    pub(super) fn clear_joint_display(&mut self) {
        self.joint.cancel.store(true, Ordering::Relaxed);
        self.joint.run_id = self.joint.run_id.wrapping_add(1);
        self.joint.running = false;
        self.joint.prepared = None;
        self.joint.progress = None;
        self.joint.measurement_run_id = None;
        self.joint.snapshot = None;
        self.joint.request_only = None;
        self.joint.report_document = None;
        self.joint.rows.clear();
        self.joint.summary.clear();
        self.joint.selected = None;
        self.joint.sort_dirty = true;
    }
    pub(super) fn restore_joint_request(&mut self, request: JointRequest) {
        self.clear_joint_display();
        self.joint.options = options_for(&request);
        self.joint.report_document = Some(request.document.clone());
        self.joint.request_only = Some(Arc::new(request));
        self.joint.show_results = true;
        self.pending_action = Some(Action::ReviewResults);
    }
    pub(super) fn has_joint_results(&self) -> bool {
        self.joint.snapshot.is_some() || self.joint.request_only.is_some()
    }
    /// Explicit source restoration may reuse saved interpretation metadata only
    /// for the exact historical review. Opening a result never calls this.
    pub(super) fn joint_catalog_for_restore(
        &self,
        document: &ReviewWorkspace,
    ) -> Option<evidence_score::LightningCatalog> {
        let request = self
            .joint
            .snapshot
            .as_ref()
            .map(|snapshot| &snapshot.request)
            .or(self.joint.request_only.as_deref())?;
        (&request.document == document).then(|| request.lightning.clone())
    }
    pub(super) fn joint_results_ui(&mut self, ui: &mut egui::Ui) -> bool {
        let Some(snapshot) = self.joint.snapshot.clone() else {
            if let Some(request) = &self.joint.request_only {
                ui.heading("Saved shared-cell request — no completed result");
                ui.colored_label(tone(ui,AMBER),"This historical run has no verified result. Its saved request is retained; missing work is not evidence against any candidate.");
                ui.label(format!(
                    "{} declared receivers · reviewed audio channel {}",
                    request.envelope_plan.receivers.len(),
                    request.observation.channel + 1
                ));
                ui.label(&request.observation.reviewed_selection_note);
                muted(
                    ui,
                    "Original sources must be loaded and checked before a new estimate or run. Opening this record did not load data, approve evidence or restart computation.",
                );
                return true;
            }
            return false;
        };
        let report = &snapshot.report;
        ui.heading("Shared-cell Doppler / thunder-envelope comparison");
        ui.label(if report.status == JointStatus::Completed {
            "Completed declared computation"
        } else {
            "Cancelled / incomplete — better alternatives may remain uncomputed"
        });
        for line in &self.joint.summary {
            ui.label(line);
        }
        if !report.ranking_available {
            ui.colored_label(tone(ui,AMBER),"Full-domain combined ranking is unavailable. Missing cells are not zero scores; the candidate domain has not been reduced.");
        }
        muted(
            ui,
            "Raw Doppler error is lower-is-better; cosine is higher-is-better. Ordinal ranks and their minimax combination are uncalibrated. Agreement does not establish source identity, and the best combined row does not resolve disagreement.",
        );
        let original = self.joint.report_document.as_ref();
        if original != self.document.as_ref() {
            ui.colored_label(tone(ui,AMBER),"Your review has changed. These immutable results still describe their original inputs and can be exported.");
        }
        if ui
            .add_enabled(
                !self.busy(),
                egui::Button::new("Save shared-cell report copy…"),
            )
            .clicked()
        {
            self.export_joint(ui.ctx());
        }
        ui.horizontal_wrapped(|ui| {
            for (mode, label) in [
                (JointView::Doppler, "Doppler"),
                (JointView::Envelope, "Thunder envelope"),
                (JointView::Combined, "Shared-cell combination"),
            ] {
                if ui
                    .selectable_value(&mut self.joint.view, mode, label)
                    .changed()
                {
                    self.joint.sort = JointColumn::Rank;
                    self.joint.descending = false;
                    self.joint.sort_dirty = true;
                }
            }
        });
        let headers = [
            (JointColumn::Rank, "Ordinal rank"),
            (JointColumn::Latitude, "Latitude"),
            (JointColumn::Longitude, "Longitude"),
            (JointColumn::Clock, "Clock offset"),
            (JointColumn::Doppler, "Doppler error"),
            (JointColumn::Envelope, "Envelope cosine"),
            (JointColumn::Status, "Availability"),
        ];
        ui.horizontal_wrapped(|ui| {
            for (column, label) in headers {
                if ui
                    .selectable_label(self.joint.sort == column, label)
                    .clicked()
                {
                    if self.joint.sort == column {
                        self.joint.descending = !self.joint.descending;
                    } else {
                        self.joint.sort = column;
                        self.joint.descending = column == JointColumn::Envelope;
                    }
                    self.joint.sort_dirty = true;
                }
            }
        });
        if self.joint.sort_dirty {
            self.joint.rows = sorted_cells(
                report,
                self.joint.view,
                self.joint.sort,
                self.joint.descending,
            );
            self.joint.sort_dirty = false;
        }
        let mut pin = None;
        egui::ScrollArea::both()
            .id_salt("joint_result_rows")
            .max_height((ui.available_height() * 0.6).clamp(120.0, 600.0))
            .show_rows(ui, 26.0, self.joint.rows.len(), |ui, range| {
                ui.set_min_width(1000.0);
                for index in range {
                    let cell_index = self.joint.rows[index];
                    let cell = &report.cells[cell_index];
                    let receiver = &report.receivers[cell.receiver_index];
                    let text = format!(
                        "{:>8}  {:>10.6}, {:>11.6}  {:+9.3}s  D {:>10}  E {:>9}  {}{}",
                        number(rank(cell, self.joint.view), 1),
                        receiver.location.latitude_deg,
                        receiver.location.longitude_deg,
                        cell.clock_offset_s,
                        number(doppler_score(cell), 5),
                        number(envelope_score(cell), 6),
                        cell_status(cell),
                        if cell.spatial_at_boundary || cell.clock_at_boundary {
                            " [limit]"
                        } else {
                            ""
                        }
                    );
                    if ui
                        .selectable_label(
                            self.joint.selected == Some(cell_index),
                            RichText::new(text).monospace(),
                        )
                        .clicked()
                    {
                        self.joint.selected = Some(cell_index);
                        pin = Some((
                            receiver.location.latitude_deg,
                            receiver.location.longitude_deg,
                        ));
                    }
                }
            });
        if let Some(index) = self.joint.selected {
            let cell = &report.cells[index];
            let receiver = &report.receivers[cell.receiver_index];
            ui.separator();
            ui.strong(format!(
                "Selected cell: {:.6}, {:.6} · {}",
                receiver.location.latitude_deg,
                receiver.location.longitude_deg,
                regional::utc_label(report.nominal_recording_start_utc_s + cell.clock_offset_s)
            ));
            let reason = state_reason(cell);
            if !reason.is_empty() {
                ui.colored_label(tone(ui, AMBER), reason);
            }
            if let DopplerState::Available {
                assignments,
                frequency_at_boundary,
                ..
            } = &cell.doppler
            {
                for assignment in assignments {
                    ui.label(format!(
                        "{} → {} · emitted tone {} Hz",
                        assignment.event_id,
                        assignment.source_id,
                        number(assignment.emitted_frequency_hz, 4)
                    ));
                }
                if *frequency_at_boundary {
                    ui.colored_label(
                        tone(ui, AMBER),
                        "Fitted emitted tone is on an approved frequency limit.",
                    );
                }
            }
            if let EnvelopeState::Available {
                contributing_source_rows,
                distance_excluded_source_rows,
                ..
            } = &cell.envelope
            {
                ui.label(format!("Envelope tile: {contributing_source_rows} contributing catalog rows; {distance_excluded_source_rows} outside distance support. This is not a per-strike association."));
            }
            muted(
                ui,
                "Selecting a row pins its coordinates for map review. It does not change the clock, sources, event selections or measurements.",
            );
        }
        if let Some((latitude, longitude)) = pin {
            self.regional_pin(latitude, longitude);
        }
        egui::CollapsingHeader::new("Saved assumptions and limitations").show(ui, |ui| {
            ui.label(format!(
                "Source query support: {} to {} (end exclusive)",
                regional::utc_label(snapshot.request.envelope_plan.catalog_utc_interval[0]),
                regional::utc_label(snapshot.request.envelope_plan.catalog_utc_interval[1])
            ));
            ui.label(format!(
                "Catalog SHA-256 {}",
                snapshot.request.lightning.input_sha256
            ));
            ui.label(&snapshot.request.observation.reviewed_selection_note);
            for limitation in &report.limitations {
                ui.label(limitation);
            }
        });
        true
    }
    fn export_joint(&mut self, ctx: &egui::Context) {
        if self.busy() {
            return;
        }
        let (Some(snapshot), Some(document)) = (
            self.joint.snapshot.clone(),
            self.joint.report_document.clone(),
        ) else {
            return;
        };
        let Some(parent) = rfd::FileDialog::new()
            .set_title("Choose a parent folder for a new immutable report copy")
            .pick_folder()
        else {
            return;
        };
        let destination = parent.join(super::super::unique_name("shared-cell-report"));
        self.spawn(
            ctx,
            "Saving the original shared-cell report and inputs…",
            move || {
                geosonnar::investigation_report::export_joint(&destination, &document, &snapshot)?;
                Ok(ResultValue::JointExported(destination))
            },
        );
    }
}

fn joint_block_reason(document: &ReviewWorkspace) -> Option<String> {
    if !document.mix.doppler_frequency || document.mix.doppler_timing || document.mix.thunder_timing
    {
        return Some("This experimental method requires Doppler frequency only: explicitly enable frequency and disable picked Doppler/thunder timing in Evidence. Your selections have not been changed.".into());
    }
    if document.atmosphere.is_some() {
        return Some("Shared-cell envelope comparison currently requires constant sound speed. Remove the atmospheric profile explicitly if you want to use this method.".into());
    }
    if document
        .envelope_review
        .as_ref()
        .and_then(|r| r.observation.as_ref())
        .is_none()
    {
        return Some("Measure a reviewed thunder-envelope interval in Audio review first.".into());
    }
    if let Err(error) = document.validate_acoustic_sources_for_scoring() {
        return Some(error);
    }
    None
}

#[cfg(test)]
#[path = "joint_tests.rs"]
mod tests;
