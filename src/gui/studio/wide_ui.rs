//! Explicit disk-backed two-pass search; map previews never supply physics.
use super::*;
use geosonnar::{
    data_size::{format_gib, format_gib_pair, gib_to_bytes},
    receiver_grid::GridLayout,
    track_store::{DEFAULT_INDEX_MEMORY_BYTES, TrackStore, TrackStoreBinding},
    wide_area_pair::{
        self, WideBudgets, WideCandidate, WideEstimate, WidePairReport, WidePairRequest,
        WideProgress,
    },
};

mod operations;
mod preparation;
mod presentation;
mod state;
#[cfg(test)]
mod tests;

use preparation::*;
use presentation::*;
pub(super) use state::{
    IndexReuseMatches, IndexReuseScope, IndexedSource, Prepared, ReusedIndexSource, WideReply,
    WideState,
};
use state::{PlanSection, Settings};

impl Studio {
    pub(super) fn wide_index_memory_ui(&mut self, ui: &mut egui::Ui) {
        ui.add_enabled_ui(!self.busy(), |ui| {
            ui.label("Source/index memory allowance (GiB)");
            data_size_input(ui, &mut self.wide.settings.max_index_memory_bytes,
                64 * 1024 * 1024..=4 * 1024 * 1024 * 1024);
            muted(ui, "Shared by indexed import, source loading and wide-area search. Separate track cache: 0.250 GiB. These are not a total application memory cap; an exhausted allowance stops the operation without reducing its scope.");
        });
    }

    fn index_matches_ui(&mut self, ui: &mut egui::Ui, scope: &IndexReuseScope) {
        let Some(found) = self
            .wide
            .index_matches
            .as_ref()
            .filter(|found| &found.scope == scope)
        else {
            return;
        };
        if found.matches.is_empty() {
            muted(
                ui,
                "No matching complete index is listed for these settings. Build a new index below.",
            );
            return;
        }
        muted(
            ui,
            "Matching saved indexes — not yet verified. Use existing index checks the current archive and saved data without rebuilding; Rebuild creates a separate copy.",
        );
        let mut selected = None;
        let enabled = !self.busy();
        let row_height = ui.spacing().interact_size.y.max(
            ui.text_style_height(&egui::TextStyle::Button) + 2. * ui.spacing().button_padding.y,
        );
        egui::ScrollArea::vertical()
            .id_salt("indexed_reuse_matches")
            .max_height(row_height * 4.)
            .show_rows(ui, row_height, found.matches.len(), |ui, rows| {
                for index in rows {
                    let matched = &found.matches[index];
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(
                                enabled,
                                egui::Button::new("Use existing index").truncate(),
                            )
                            .clicked()
                        {
                            selected = Some(matched.clone());
                        }
                        let label = format!(
                            "{} · {} · {}",
                            matched.label,
                            format_gib(matched.registered_bytes),
                            matched.catalog_id
                        );
                        ui.add(egui::Label::new(&label).truncate())
                            .on_hover_text(label);
                    });
                }
            });
        if let Some(selected) = selected {
            self.use_index_match(ui.ctx(), scope.clone(), selected);
        }
    }

    pub(super) fn indexed_archive_ui(
        &mut self,
        ui: &mut egui::Ui,
        entry: &data_catalog::CatalogEntry,
    ) {
        ui.separator();
        ui.strong("Wide-area search data — keep all selected points on disk");
        muted(
            ui,
            "This indexed copy can exceed two million points. It stays separate from the display map and preserves the requested full-day scope. Source, altitude and gap settings above also apply.",
        );
        self.wide_index_memory_ui(ui);
        ui.add_enabled_ui(!self.busy(), |ui| {
            if ui.button("Use complete archive UTC day").clicked() && let Some(date) = &entry.date_utc
                && let Ok(start) = geosonnar::detailed_data::utc_midnight(date) {
                self.wide.import_start = utc_input(start);
                self.wide.import_end = utc_input(start + 86400.);
            }
            ui.label("UTC source interval (blank fields use the complete archive day)");
            accessible_text(ui, "wide_import_start", &mut self.wide.import_start, "YYYY-MM-DD HH:MM:SS", false, ui.available_width());
            accessible_text(ui, "wide_import_end", &mut self.wide.import_end, "YYYY-MM-DD HH:MM:SS", false, ui.available_width());
            ui.add(egui::DragValue::new(&mut self.wide.import_padding_km).range(0.0..=500.0).suffix(" km aircraft-area padding"));
            ui.label("Indexed-data size allowance (GiB)");
            ui.add(egui::DragValue::new(&mut self.wide.import_disk_gib).range(0.1..=1024.0).min_decimals(3).max_decimals(10).suffix(" GiB"));
            muted(ui, "This size limit applies to a new copy or the size of a saved index. Reusing an index needs no duplicate copy; source/index memory is checked separately.");
            muted(ui, "The UTC interval is explicit: allow propagation/interpolation look-back when narrowing it. Adjacent-day data require their own archive selection; this action never silently clips or expands time.");
            let scope = self.index_reuse_scope(entry);
            if ui.add_enabled(scope.is_ok(), egui::Button::new("Find matching index")).clicked()
                && let Ok(scope) = &scope {
                self.find_index_matches(ui.ctx(), scope.clone());
            }
            if let Ok(scope) = &scope { self.index_matches_ui(ui, scope); }
            let has_match = scope.as_ref().ok().is_some_and(|scope| self.wide.index_matches.as_ref()
                .is_some_and(|found| &found.scope == scope && !found.matches.is_empty()));
            let build_label = if has_match { "Rebuild a new index" } else { "Build indexed flight data" };
            if ui.add_enabled(!self.busy(), egui::Button::new(build_label)).clicked() {
                let prepared = self.indexed_import_config(entry, &self.wide.import_start, &self.wide.import_end, self.wide.import_padding_km);
                match prepared {
                    Err(error) => self.notice = Some((true, error)),
                    Ok(config) => {
                        let manifest = self.case.as_ref().unwrap().manifest.clone(); let id = entry.id.clone();
                        let bytes = match gib_to_bytes(self.wide.import_disk_gib) {
                            Ok(bytes) => bytes,
                            Err(error) => { self.notice = Some((true, error)); return; }
                        };
                        let memory = self.wide.settings.max_index_memory_bytes;
                        self.wide.cancel = Arc::new(AtomicBool::new(false));
                        self.download_cancel = self.wide.cancel.clone(); let cancel = self.wide.cancel.clone();
                        self.start_wide_task(ui.ctx(), "Scanning archive and building a bounded disk index…", move || {
                            data_catalog::import_adsblol_indexed_with_limits(&manifest, &id, &config, bytes, memory, &cancel)
                                .map(|entry| ResultValue::WideReady(Box::new(WideReply::Imported(Box::new(entry)))))
                        });
                    }
                }
            }
        });
    }

    pub(super) fn wide_progress_ui(&mut self, ui: &mut egui::Ui) {
        if !self.wide.running {
            return;
        }
        ui.horizontal_wrapped(|ui| {
            ui.label(format!(
                "Wide-area work · {:.0}s",
                self.wide.started.map_or(0., |s| s.elapsed().as_secs_f64())
            ));
            if self.wide.cancel.load(Ordering::Relaxed) {
                ui.label("Cancellation requested; waiting for the current validation checkpoint. Saved-page validation may finish before cancellation is applied.");
            } else if ui.button("Cancel wide-area work").clicked() {
                self.wide.cancel.store(true, Ordering::Relaxed);
            }
        });
        if let Some(progress) = &self.wide.progress {
            ui.strong(wide_progress_stage_label(&progress.stage));
            ui.label(format!(
                "Finished locations: {} / {} · {} remaining",
                progress.completed_receivers,
                progress.total_receivers,
                progress
                    .total_receivers
                    .saturating_sub(progress.completed_receivers)
            ));
            ui.label(format!(
                "Timing-compatible pairs tried: {} · Valid pair scores: {}",
                progress.timing_candidates, progress.scored_candidates
            ));
            score_bound_progress_ui(ui, progress.score_bound_pruned_candidates);
            ui.label(format!(
                "Flight passes examined: {}",
                progress.scanned_passes
            ));
            if let Some(active) = &progress.active_receiver {
                ui.label(format!(
                    "Current location: {} / {}",
                    active.receiver_index.saturating_add(1),
                    progress.total_receivers
                ));
                if let Some(total) = active.eligible_pair_total {
                    ui.label(format!(
                        "Pairs tried at this location: {} / {}",
                        active.attempted_pairs, total
                    ));
                } else {
                    muted(ui, "Finding and counting flight pairs at this location.");
                }
            }
            if progress.completed_receivers < progress.total_receivers
                && (matches!(
                    progress.stage.as_str(),
                    "local_cpa_census" | "shared_clock_pair_scoring"
                ) || progress.timing_candidates > 0)
            {
                muted(
                    ui,
                    "Pair counts may include work on an unfinished location. A location enters results only after all its eligible pairs are examined.",
                );
            }
        }
        ui.ctx().request_repaint_after(Duration::from_millis(200));
    }

    pub(super) fn wide_controls_ui(&mut self, ui: &mut egui::Ui) {
        ui.heading("Wide-area two-pass Doppler");
        muted(
            ui,
            "Full indexed trajectories and all local CPA alternatives. This separate method uses two reviewed frequency traces and their time separation; it does not combine thunder or change your evidence selections.",
        );
        if let Some(source) = &self.wide.source {
            ui.label(format!(
                "{} · {} passes · {} points on disk",
                source.label, source.passes, source.points
            ));
            ui.label(format!(
                "Source interval: {} — {} · maximum interpolation gap {} s",
                regional::utc_label(source.utc_window[0]),
                regional::utc_label(source.utc_window[1]),
                source.max_gap_s
            ));
        } else {
            ui.colored_label(tone(ui, AMBER), "Data → select a complete day archive → Find matching index → Use existing index, or Build indexed flight data. A stored indexed dataset can also be selected directly.");
        }
        ui.add_enabled_ui(!self.busy(), |ui| {
            let _ = plan_section_tabs(ui, &mut self.wide.section);
            ui.separator();
            if self.document.is_none() {
                muted(ui, "Open a case to prepare a wide-area plan.");
                return;
            }
            match self.wide.section {
                PlanSection::Area => {
                    let doc = self.document.as_mut().unwrap();
            ui.strong("Area/grid");
            for (label, value) in ["South", "West", "North", "East"].into_iter().zip(&mut doc.map.bounds) {
                ui.horizontal(|ui| { ui.label(label); ui.add(egui::DragValue::new(value).speed(0.01)); });
            }
            egui::ComboBox::from_id_salt("wide_spacing").selected_text(format!("{} m", doc.map.grid_spacing_m)).show_ui(ui, |ui| {
                for value in [100_000., 50_000., 25_000., 10_000., 2000., 1000., 500., 100., 25.] {
                    ui.selectable_value(&mut doc.map.grid_spacing_m, value, format!("{value} m"));
                }
            });
            ui.add(egui::DragValue::new(&mut doc.map.grid_spacing_m).range(10.0..=100_000.0).suffix(" m maximum spacing"));
            match GridLayout::new(&doc.map) {
                Ok(grid) => { ui.label(format!("{} search locations · {} rows × {} columns", grid.count, grid.rows, grid.columns)); }
                Err(error) => { ui.colored_label(tone(ui, RED), error); }
            }
            accessible_text(ui, "wide_clock_start", &mut self.wide.settings.utc_start, "Earliest recording start UTC", false, ui.available_width());
            accessible_text(ui, "wide_clock_end", &mut self.wide.settings.utc_end, "Latest recording start UTC", false, ui.available_width());
            ui.add(egui::DragValue::new(&mut doc.map.receiver_height_m).suffix(" m receiver ellipsoid height"));
            muted(ui, "A location is one candidate microphone position (receiver). Every declared location remains in the search; the map preview may show fewer points.");
                }
                PlanSection::Evidence => {
                    let doc = self.document.as_mut().unwrap();
            ui.strong("Evidence/assumptions");
            ui.add(egui::DragValue::new(&mut doc.sound_speed_mps).suffix(" m/s sound speed"));
            muted(ui, "This pass uses the explicit constant receiver height and sound speed. Terrain/atmospheric refinement remains a separate reviewed comparison.");
            if ui.button("Copy the two selected reviewed Doppler events").clicked()
                && let Err(error) = copy_reviewed_pair(doc, &mut self.wide.settings) {
                self.notice = Some((true, error));
            }
            muted(ui, "Copy updates timing and tone suggestions. Each event's physical limits stay as entered; review them when changing the selected pair.");
            for i in 0..2 {
                ui.push_id(("wide_event", i), |ui| {
                    ui.strong(format!("Event {}: {}", i + 1, self.wide.settings.event_ids[i]));
                    ui.add(egui::DragValue::new(&mut self.wide.settings.cpa_s[i]).suffix(" s CPA anchor").speed(0.1));
                    range_ui(ui, "Emitted tone grid — review the suggested range", &mut self.wide.settings.tones[i], " Hz");
                    physical_range_ui(ui, &format!("Event {} speed", i + 1), &mut self.wide.settings.speed[i], " m/s", 1.);
                    physical_range_ui(ui, &format!("Event {} slant distance", i + 1), &mut self.wide.settings.distance[i], " m", 100.);
                });
            }
            range_ui(ui, "Shared shift around CPA-derived clock", &mut self.wide.settings.shift, " s");
            ui.add(egui::DragValue::new(&mut self.wide.settings.separation_tolerance_s).range(0.0..=3600.0).suffix(" s pair-separation tolerance"));
                }
                PlanSection::Resources => {
                    ui.strong("Resource allowances");
                    let locations = self.document.as_ref().and_then(|doc| GridLayout::new(&doc.map).ok()).map(|grid| grid.count);
                    output_allowance_ui(ui, &mut self.wide.settings, locations);
                    ui.separator();
                    self.wide_index_memory_ui(ui);
                    if run_disabled_reason(&self.wide, self.document.as_ref(), false).is_none()
                        && let Some(prepared) = &self.wide.prepared {
                        let (required, allowed) = format_gib_pair(prepared.estimate.required_index_memory_bytes, self.wide.settings.max_index_memory_bytes);
                        ui.label(format!("Estimated source/index requirement: {required} · Allowed: {allowed}"));
                    } else {
                        muted(ui, "A current estimate is needed for the source/index memory requirement.");
                    }
                    ui.separator();
                    runtime_and_work_ui(ui, &mut self.wide.settings);
                    if self.wide.settings.work_limit_enabled && let Some(locations) = locations
                        && locations > self.wide.settings.max_work {
                        ui.colored_label(tone(ui, RED), format!("Work-unit allowance is too small: at least {locations} units for {locations} locations; allowed {}. Increase the work-unit allowance or explicitly turn that limit off, then estimate again.", self.wide.settings.max_work));
                    }
                    ui.label("Retained ranking rows");
                    ui.add(egui::DragValue::new(&mut self.wide.settings.retain).range(1..=1000));
                    muted(ui, "Retained ranking rows limits the compact shortlists. It does not reduce the locations searched or remove the need to save location summaries.");
                }
                PlanSection::Estimate => {
                    ui.strong("Estimate/run");
                    if let Some(prepared) = &self.wide.prepared {
                        let current = run_disabled_reason(&self.wide, self.document.as_ref(), false).is_none();
                        if !current { ui.colored_label(tone(ui, AMBER), "Previous estimate — settings changed. Estimate again before using these figures."); }
                        let e = &prepared.estimate;
                        ui.label(format!("{} locations · {} source passes · {} source points", e.receiver_count, e.source_passes, e.source_points));
                        ui.label(format!("Unpruned ceiling: {} pass checks / {} point visits. Filtering determines actual pair work.", e.unpruned_pass_checks, e.worst_scan_point_visits));
                        ui.label(format!("Preliminary minimum output reservation: {}", format_gib(e.minimum_output_bytes)));
                        ui.label(format!("Source/index requirement: {} total · {} source/verification · {} spatial index · {} local CPA reserve. Separate track cache: 0.250 GiB.", format_gib(e.required_index_memory_bytes), format_gib(e.source_metadata_bytes), format_gib(e.spatial_index_memory_bytes), format_gib(e.local_cpa_memory_reserve_bytes)));
                        if current { cost_preview_ui(ui, e); }
                    } else {
                        muted(ui, "No verified estimate yet. Review Area/grid, Evidence/assumptions and Resource allowances, then estimate the complete plan.");
                    }
                    muted(ui, "Preliminary minimum reservations are not forecasts of actual output or completion time. Passing the estimate does not guarantee the whole search will finish.");
                    if ui.button("Review Resource allowances").clicked() { self.wide.section = PlanSection::Resources; }
                }
            }
            ui.separator();
            ui.horizontal_wrapped(|ui| {
                if ui.button("Estimate complete wide-area plan").clicked() { self.prepare_wide(ui.ctx(), false); }
                let ready = run_disabled_reason(&self.wide, self.document.as_ref(), self.busy()).is_none();
                if ui.add_enabled(ready, egui::Button::new("Run reviewed wide-area plan")).clicked() { self.prepare_wide(ui.ctx(), true); }
            });
            if let Some(reason) = run_disabled_reason(&self.wide, self.document.as_ref(), self.busy()) {
                ui.colored_label(tone(ui, AMBER), reason);
            }
            muted(ui, "The runtime limit can stop the search before all locations finish. Output or memory limits and cancellation can also leave an incomplete run. Completed-location summaries are saved; there is no automatic retry or resume. A separate review snapshot (up to 0.0078125 GiB) is saved before computation.");
        });
    }

    pub(super) fn wide_results_ui(&mut self, ui: &mut egui::Ui) -> bool {
        if self.wide.report.is_some() {
            ui.horizontal_wrapped(|ui| {
                ui.selectable_value(&mut self.wide.show_results, true, "Wide-area results");
                ui.selectable_value(&mut self.wide.show_results, false, "Other results");
            });
        }
        if ui
            .add_enabled(!self.busy(), egui::Button::new("Open saved wide-area run…"))
            .clicked()
            && let Some(path) = rfd::FileDialog::new().pick_folder()
        {
            self.wide.cancel = Arc::new(AtomicBool::new(false));
            let cancel = self.wide.cancel.clone();
            self.start_wide_task(
                ui.ctx(),
                "Opening the saved wide-area result…",
                move || {
                    wide_checkpoint(&cancel)?;
                    let reply = load_saved_wide(&path)?;
                    wide_checkpoint(&cancel)?;
                    Ok(ResultValue::WideReady(Box::new(reply)))
                },
            );
        }
        if !self.wide.show_results {
            return false;
        }
        let Some(report) = self.wide.report.clone() else {
            return false;
        };
        ui.heading("Wide-area two-pass results");
        if self.wide.report_source_missing {
            ui.colored_label(tone(ui, AMBER), "Historical result: its indexed source was deleted or became unavailable. Scores are retained; source verification and refinement are unavailable.");
        }
        ui.label(format!(
            "{:?} · {} / {} receivers · {} remaining",
            report.status,
            report.progress.completed_receivers,
            report.progress.total_receivers,
            report
                .progress
                .total_receivers
                .saturating_sub(report.progress.completed_receivers)
        ));
        ui.label(format!("Saved: {}", display_path(&report.output_directory)));
        score_bound_progress_ui(ui, report.progress.score_bound_pruned_candidates);
        if let Some(error) = &report.error {
            saved_diagnostic_ui(
                ui,
                error,
                report.progress.total_receivers,
                self.wide.report_budgets.as_ref(),
            );
        }
        if let Some(guidance) = stop_guidance(&report) {
            if report.progress.completed_receivers == 0 {
                ui.colored_label(tone(ui, AMBER), "No receiver finished. This is not a no-match result; unfinished receiver scores are excluded from the completed results.");
                if report.progress.scored_candidates > 0 {
                    ui.label(format!("{} provisional pair scores were attempted before stopping; no completed receiver ranking is available.", report.progress.scored_candidates));
                }
            }
            ui.label(guidance);
        }
        muted(
            ui,
            "Partial runs show provisional alternatives, not a finished winner. Frequency RMSE is uncalibrated; source coverage and location are not verified. Map/display sampling never supplies these scores.",
        );
        if !report.source_verified_at_end {
            ui.colored_label(tone(ui, AMBER), "The source did not pass final verification; treat this result as provisional and inspect the saved limitations.");
        }
        if report.retained_truncated || report.pair_retention_truncated {
            muted(
                ui,
                "The compact table is truncated; the run folder retains the processed-receiver records. These rows do not establish uniqueness.",
            );
        }
        egui::CollapsingHeader::new(format!("Best retained flight pairs ({})", report.best_flight_pairs.len())).show(ui, |ui| {
            if report.best_flight_pairs.len() > 100 { muted(ui, "Showing the first 100 retained pairs; the saved report contains the complete retained list."); }
            for candidate in report.best_flight_pairs.iter().take(100) { ui.label(format!("{} + {} · {:.4} Hz · {:.5}, {:.5}", candidate.score.flight_ids[0], candidate.score.flight_ids[1], candidate.score.combined_rmse_hz, candidate.observer.latitude_deg, candidate.observer.longitude_deg)); }
        });
        ui.horizontal_wrapped(|ui| {
            for (column, name) in ["RMSE", "Latitude", "Longitude", "Clock"]
                .into_iter()
                .enumerate()
            {
                if ui.button(name).clicked() {
                    self.wide.sort_reverse =
                        self.wide.sort_column == column && !self.wide.sort_reverse;
                    self.wide.sort_column = column;
                    let value = |c: &WideCandidate| match column {
                        1 => c.observer.latitude_deg,
                        2 => c.observer.longitude_deg,
                        3 => c.score.clock_offset_s,
                        _ => c.score.combined_rmse_hz,
                    };
                    self.wide.row_order.sort_by(|a, b| {
                        value(&report.best_hypotheses[*a])
                            .total_cmp(&value(&report.best_hypotheses[*b]))
                    });
                    if self.wide.sort_reverse {
                        self.wide.row_order.reverse();
                    }
                }
            }
        });
        let mut refine = None;
        let row_height = wide_result_row_height(ui);
        egui::ScrollArea::vertical()
            .id_salt("wide_result_rows")
            .show_rows(ui, row_height, self.wide.row_order.len(), |ui, rows| {
                for row in rows {
                    let candidate = &report.best_hypotheses[self.wide.row_order[row]];
                    let summary = format!(
                        "{:.4} Hz · {:.6}, {:.6} · {} · {} + {}",
                        candidate.score.combined_rmse_hz,
                        candidate.observer.latitude_deg,
                        candidate.observer.longitude_deg,
                        report
                            .source_time_origin_utc_s
                            .map(|origin| regional::utc_label(
                                origin + candidate.score.clock_offset_s
                            ))
                            .unwrap_or_else(
                                || "UTC unavailable: source origin was not verified".into()
                            ),
                        candidate.score.flight_ids[0],
                        candidate.score.flight_ids[1],
                    );
                    let can_refine = !self.busy()
                        && !self.wide.report_source_missing
                        && report.source_verified_at_end
                        && self.wide.report_document.is_some()
                        && self.wide.report_document.as_ref() == self.document.as_ref()
                        && self
                            .wide
                            .source
                            .as_ref()
                            .is_some_and(|source| source.binding == report.source);
                    let id = ui.make_persistent_id((
                        "wide_result",
                        candidate.receiver_index,
                        candidate.encounter_index,
                    ));
                    let (button, _) = wide_result_row(ui, id, &summary, can_refine);
                    if button.clicked() {
                        refine = Some(candidate.observer.clone());
                    }
                }
            });
        if let Some(observer) = refine
            && let Some(before) = self.document.clone()
        {
            let dlat = 1000. / 111_320.;
            let dlon = dlat / observer.latitude_deg.to_radians().cos().abs().max(0.01);
            let doc = self.document.as_mut().unwrap();
            doc.map.bounds = [
                observer.latitude_deg - dlat,
                observer.longitude_deg - dlon,
                observer.latitude_deg + dlat,
                observer.longitude_deg + dlon,
            ];
            doc.map.grid_spacing_m = 100.;
            self.checkpoint(before);
            self.wide.prepared = None;
            self.wide.enabled = true;
            self.notice = Some((false, "Prepared a 1 km half-width, 100 m local grid. Review it in Search and estimate again. The full source pool and event approvals were preserved.".into()));
        }
        true
    }
}
