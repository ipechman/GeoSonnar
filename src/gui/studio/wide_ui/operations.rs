//! Explicit jobs, source reuse and result application.
use super::*;

impl Studio {
    pub(in crate::gui::studio) fn wide_dependency_paths(
        &self,
    ) -> impl Iterator<Item = &std::path::Path> {
        self.wide
            .source
            .iter()
            .map(|source| source.binding.manifest_path.as_path())
            .chain(
                self.wide
                    .report
                    .iter()
                    .map(|report| report.source.manifest_path.as_path()),
            )
    }
    pub(in crate::gui::studio) fn wide_report_document(&self) -> Option<&ReviewWorkspace> {
        self.wide.report_document.as_ref()
    }
    pub(in crate::gui::studio) fn invalidate_wide_source(
        &mut self,
        affected: impl Fn(&std::path::Path) -> bool,
    ) {
        if let Some(matches) = &mut self.wide.index_matches {
            matches
                .matches
                .retain(|item| !affected(&item.index.manifest_path));
        }
        if self
            .wide
            .source
            .as_ref()
            .is_some_and(|s| affected(&s.binding.manifest_path))
        {
            self.wide.source = None;
            self.wide.prepared = None;
        }
        if self
            .wide
            .report
            .as_ref()
            .is_some_and(|r| affected(&r.source.manifest_path))
        {
            self.wide.report_source_missing = true;
        }
    }
    pub(in crate::gui::studio) fn start_wide_task<F>(
        &mut self,
        ctx: &egui::Context,
        label: &str,
        task: F,
    ) where
        F: FnOnce() -> Result<ResultValue, String> + Send + 'static,
    {
        self.wide.run_id = self.wide.run_id.wrapping_add(1);
        self.wide.running = true;
        self.wide.started = Some(Instant::now());
        self.wide.progress = None;
        self.busy = Some(label.into());
        self.notice = None;
        self.spawn_lane(ctx, WorkerLane::WideTask(self.wide.run_id), task);
    }

    pub(in crate::gui::studio) fn index_reuse_scope(
        &self,
        entry: &data_catalog::CatalogEntry,
    ) -> Result<IndexReuseScope, String> {
        if entry.format != "adsblol_archive" {
            return Err("Select a complete downloaded day archive".into());
        }
        let acquisition_manifest_sha256 = entry
            .files
            .iter()
            .find(|file| file.relative_path == entry.primary_file)
            .ok_or("The archive has no registered manifest identity")?
            .sha256
            .clone();
        let config = self.indexed_import_config(
            entry,
            &self.wide.import_start,
            &self.wide.import_end,
            self.wide.import_padding_km,
        )?;
        let canonical_config = serde_json::to_string(&config).map_err(|error| error.to_string())?;
        Ok(IndexReuseScope {
            source_catalog_id: entry.id.clone(),
            acquisition_manifest_sha256,
            config,
            canonical_config,
        })
    }

    pub(in crate::gui::studio) fn index_reuse_scope_current(
        &self,
        scope: &IndexReuseScope,
    ) -> bool {
        self.data
            .selected_item()
            .filter(|item| {
                item.entry.id == scope.source_catalog_id
                    && item.status == "available"
                    && item.problem.is_none()
            })
            .and_then(|item| self.index_reuse_scope(&item.entry).ok())
            .as_ref()
            == Some(scope)
    }

    pub(in crate::gui::studio) fn find_index_matches(
        &mut self,
        ctx: &egui::Context,
        scope: IndexReuseScope,
    ) {
        if self.busy() {
            return;
        }
        let Some(case) = &self.case else {
            return;
        };
        let manifest = case.manifest.clone();
        self.wide.index_matches = None;
        self.wide.cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.wide.cancel.clone();
        self.start_wide_task(
            ctx,
            "Finding saved indexes for these archive settings…",
            move || {
                let matches = data_catalog::find_completed_indexed_with_cancel(
                    &manifest,
                    &scope.source_catalog_id,
                    &scope.config,
                    &cancel,
                )?;
                wide_checkpoint(&cancel)?;
                Ok(ResultValue::WideReady(Box::new(WideReply::IndexMatches(
                    Box::new(IndexReuseMatches { scope, matches }),
                ))))
            },
        );
    }

    pub(in crate::gui::studio) fn use_index_match(
        &mut self,
        ctx: &egui::Context,
        scope: IndexReuseScope,
        matched: data_catalog::IndexedReuseMatch,
    ) {
        if self.busy() || !self.index_reuse_scope_current(&scope) {
            return;
        }
        let (Some(case), Some(document)) = (&self.case, self.document.clone()) else {
            return;
        };
        let manifest = case.manifest.clone();
        let bytes = match gib_to_bytes(self.wide.import_disk_gib) {
            Ok(bytes) => bytes,
            Err(error) => {
                self.notice = Some((true, error));
                return;
            }
        };
        let memory = self.wide.settings.max_index_memory_bytes;
        self.wide.cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.wide.cancel.clone();
        self.start_wide_task(
            ctx,
            "Verifying the archive and saved index; no new copy is being built…",
            move || {
                let verified = data_catalog::verify_indexed_reuse_with_limits(
                    &manifest,
                    &scope.source_catalog_id,
                    &matched,
                    &scope.config,
                    bytes,
                    memory,
                    &cancel,
                )?;
                let source = IndexedSource {
                    binding: verified.binding,
                    label: verified.matched.label,
                    passes: usize::try_from(verified.source_passes)
                        .map_err(|_| "Indexed pass count exceeds this platform")?,
                    points: verified.source_points,
                    origin: verified.time_origin_utc_s,
                    utc_window: verified.config.time_window_utc_s,
                    max_gap_s: verified.config.max_gap_s,
                };
                wide_checkpoint(&cancel)?;
                Ok(ResultValue::WideReady(Box::new(WideReply::ReusedSource(
                    Box::new(ReusedIndexSource {
                        scope,
                        source,
                        document: Box::new(document),
                    }),
                ))))
            },
        );
    }

    pub(in crate::gui::studio) fn load_indexed_source(
        &mut self,
        ctx: &egui::Context,
        entry: &data_catalog::CatalogEntry,
    ) {
        if self.busy() {
            return;
        }
        let (Some(case), Some(previous)) = (&self.case, self.document.clone()) else {
            return;
        };
        let manifest = case.manifest.clone();
        let id = entry.id.clone();
        let primary = entry.primary_file.clone();
        let label = entry.label.clone();
        let memory = self.wide.settings.max_index_memory_bytes;
        self.wide.cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.wide.cancel.clone();
        self.start_wide_task(
            ctx,
            "Verifying the complete indexed flight source…",
            move || {
                wide_checkpoint(&cancel)?;
                let path = data_catalog::verified_files_with_cancel(&manifest, &id, &cancel)?
                    .into_iter()
                    .find(|p| p.ends_with(primary.as_path()))
                    .ok_or("Index primary file is missing")?;
                let store =
                    TrackStore::open_with_memory_limit(&path, memory, || wide_checkpoint(&cancel))?;
                let source = IndexedSource {
                    binding: store.binding().clone(),
                    label,
                    passes: store.passes().len(),
                    points: store.total_points(),
                    origin: store.time_origin_utc_s(),
                    utc_window: store.import_config().time_window_utc_s,
                    max_gap_s: store.import_config().max_gap_s,
                };
                wide_checkpoint(&cancel)?;
                Ok(ResultValue::WideReady(Box::new(WideReply::Source(
                    source,
                    Box::new(previous),
                ))))
            },
        );
    }

    pub(in crate::gui::studio) fn accept_wide_reply(&mut self, reply: WideReply) {
        self.wide.running = false;
        if self.wide.cancel.load(Ordering::Relaxed)
            && matches!(
                &reply,
                WideReply::Source(..)
                    | WideReply::Estimated(..)
                    | WideReply::ReopenedReport(..)
                    | WideReply::IndexMatches(..)
                    | WideReply::ReusedSource(..)
            )
        {
            self.notice = Some((false, "Wide-area verification cancelled; no source selection, new estimate or reopened report was applied.".into()));
            return;
        }
        match reply {
            WideReply::IndexMatches(found) => {
                if !self.index_reuse_scope_current(&found.scope)
                    || found.matches.iter().any(|matched| {
                        matched.source_catalog_id != found.scope.source_catalog_id
                            || matched.acquisition_manifest_sha256
                                != found.scope.acquisition_manifest_sha256
                    })
                {
                    self.notice = Some((true, "Archive or import settings changed while finding indexes; check again for the current settings.".into()));
                    return;
                }
                self.notice = Some((
                    false,
                    format!(
                        "{} matching saved index(es) listed. Use existing index verifies the source and data; nothing has been loaded or built.",
                        found.matches.len()
                    ),
                ));
                self.wide.index_matches = Some(found);
            }
            WideReply::ReusedSource(reused) => {
                if !self.index_reuse_scope_current(&reused.scope) {
                    self.notice = Some((true, "Archive or import settings changed during verification; the saved index was not selected. Check again for the current settings.".into()));
                    return;
                }
                self.accept_wide_reply(WideReply::Source(reused.source, reused.document));
            }
            WideReply::Imported(entry) => {
                self.wide.index_matches = None;
                self.data.select_entry(entry.id.clone());
                self.data.refresh_requested = true;
                self.notice = Some((
                    false,
                    format!(
                        "{} is ready. Select Use indexed flight data to include it in a wide-area search; no map points or events were selected automatically.",
                        entry.label
                    ),
                ));
            }
            WideReply::Source(source, previous) => {
                if self.document.as_ref() != Some(previous.as_ref()) {
                    self.notice = Some((
                        true,
                        "Review changed while checking indexed data; select the source again."
                            .into(),
                    ));
                    return;
                }
                let doc = self.document.as_mut().unwrap();
                if let Err(error) = doc.register_dataset(
                    SourceBinding {
                        path: source.binding.manifest_path.clone(),
                        sha256: source.binding.manifest_sha256.clone(),
                    },
                    Some(source.origin),
                    false,
                ) {
                    self.notice = Some((true, error));
                    return;
                }
                self.checkpoint(*previous);
                self.restore_deleted_source(&SourceBinding {
                    path: source.binding.manifest_path.clone(),
                    sha256: source.binding.manifest_sha256.clone(),
                });
                self.wide.settings.utc_start = utc_input(source.utc_window[0]);
                self.wide.settings.utc_end = utc_input(source.utc_window[1]);
                self.wide.source = Some(source);
                self.wide.prepared = None;
                self.notice = Some((false, "Indexed flight data selected. Open Search → Wide-area two-pass Doppler. Map previews are not used for scoring.".into()));
            }
            WideReply::Estimated(prepared) => {
                if self.document.as_ref() == Some(&prepared.document)
                    && self.wide.settings == prepared.settings
                    && self
                        .wide
                        .source
                        .as_ref()
                        .is_some_and(|s| s.binding == prepared.request.source)
                {
                    self.wide.prepared = Some(prepared);
                } else {
                    self.notice = Some((
                        true,
                        "Review or search options changed; estimate the current plan again.".into(),
                    ));
                }
            }
            WideReply::Report(report, document, budgets)
            | WideReply::ReopenedReport(report, document, budgets) => {
                self.wide.row_order = (0..report.best_hypotheses.len()).collect();
                self.wide.sort_column = 0;
                self.wide.sort_reverse = false;
                self.wide.report_source_missing = document
                    .as_deref()
                    .is_some_and(|doc| !self.deleted_sources_ready(doc));
                self.wide.report = Some(Arc::new(*report));
                self.wide.report_document = document.map(|d| *d);
                self.wide.report_budgets = budgets;
                self.wide.show_results = true;
                self.pending_action = Some(Action::ReviewResults);
                self.notice = Some((false, "Wide-area result saved. Review its completion status and alternatives; nothing was applied to the evidence.".into()));
            }
        }
    }

    pub(in crate::gui::studio) fn prepare_wide(&mut self, ctx: &egui::Context, run: bool) {
        if self.busy() || self.exiting {
            return;
        }
        let (Some(case), Some(doc), Some(source)) =
            (&self.case, self.document.clone(), self.wide.source.clone())
        else {
            self.notice = Some((true, "Select indexed flight data and a case first".into()));
            return;
        };
        if !self.deleted_sources_ready(&doc) {
            self.notice = Some((
                true,
                "Restore the deleted original source before running this review".into(),
            ));
            return;
        }
        if doc.receiver_terrain.is_some() || doc.atmosphere.is_some() {
            self.notice = Some((true, "This wide-area method uses constant receiver height and sound speed. Keep your current terrain/atmospheric review and prepare a separate plain-height review explicitly.".into()));
            return;
        }
        let settings = self.wide.settings.clone();
        let manifest = case.manifest.clone();
        let request = match request(&doc, &source, &settings) {
            Ok(v) => v,
            Err(e) => {
                self.notice = Some((true, e));
                return;
            }
        };
        if let Err(error) =
            wide_area_pair::check_preliminary_budgets(request.grid.count, &request.budgets)
        {
            self.wide.section = PlanSection::Resources;
            self.notice = Some((true, error));
            return;
        }
        self.wide.cancel = Arc::new(AtomicBool::new(false));
        if !run {
            let cancel = self.wide.cancel.clone();
            let sender = self.sender.clone();
            let generation = self.generation;
            let run_id = self.wide.run_id.wrapping_add(1);
            let repaint = ctx.clone();
            self.start_wide_task(
                ctx,
                "Checking the entire indexed source and wide-area plan…",
                move || {
                    workspace::verify_with_cancel(&manifest, &doc, &cancel)?;
                    let mut last = Instant::now() - Duration::from_secs(1);
                    let estimate =
                        wide_area_pair::estimate_with_cancel(&request, &cancel, |progress| {
                            if last.elapsed() >= Duration::from_millis(150) {
                                let _ = sender.send(Reply {
                                    generation,
                                    lane: WorkerLane::SearchProgress,
                                    result: Ok(ResultValue::WideProgress(run_id, progress)),
                                });
                                repaint.request_repaint();
                                last = Instant::now();
                            }
                        })?;
                    workspace::verify_with_cancel(&manifest, &doc, &cancel)?;
                    wide_checkpoint(&cancel)?;
                    Ok(ResultValue::WideReady(Box::new(WideReply::Estimated(
                        Box::new(Prepared {
                            request,
                            estimate,
                            document: doc,
                            settings,
                        }),
                    ))))
                },
            );
            return;
        }
        if self.wide.prepared.as_ref().is_none_or(|p| {
            p.document != doc || p.settings != settings || p.request.source != source.binding
        }) {
            self.notice = Some((true, "Estimate the current plan before running".into()));
            return;
        }
        let cancel = self.wide.cancel.clone();
        let sender = self.sender.clone();
        let generation = self.generation;
        let run_id = self.wide.run_id.wrapping_add(1);
        let repaint = ctx.clone();
        self.start_wide_task(
            ctx,
            "Searching full-resolution indexed trajectories…",
            move || {
                workspace::verify_with_cancel(&manifest, &doc, &cancel)?;
                let root = manifest
                    .parent()
                    .ok_or("Case has no directory")?
                    .join("wide-searches");
                ordinary_result_parents(manifest.parent().ok_or("Case has no directory")?)?;
                if !root.try_exists().map_err(|e| e.to_string())? {
                    std::fs::create_dir(&root).map_err(|e| e.to_string())?;
                }
                ordinary_result_parents(&root)?;
                let name = format!(
                    "run-{}-{}",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_err(|e| e.to_string())?
                        .as_nanos()
                );
                let output = root.join(&name);
                save_review_snapshot(&output, &request, &doc)?;
                let mut last = Instant::now() - Duration::from_secs(1);
                let report = wide_area_pair::run(&request, &output, &cancel, |progress| {
                    if last.elapsed() >= Duration::from_millis(150) || progress.stage == "finished"
                    {
                        let _ = sender.send(Reply {
                            generation,
                            lane: WorkerLane::SearchProgress,
                            result: Ok(ResultValue::WideProgress(run_id, progress)),
                        });
                        repaint.request_repaint();
                        last = Instant::now();
                    }
                })
                .map_err(|error| {
                    format!(
                        "{error}; preserved wide-area run: {}",
                        display_path(&output)
                    )
                })?;
                Ok(ResultValue::WideReady(Box::new(WideReply::Report(
                    Box::new(report),
                    Some(Box::new(doc)),
                    Some(request.budgets.clone()),
                ))))
            },
        );
    }
}
