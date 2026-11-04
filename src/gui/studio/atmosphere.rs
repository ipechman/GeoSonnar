//! Profile intake is explicit; the complete validated profile is saved in the review.
use super::*;

impl Studio {
    pub(super) fn atmosphere_ui(&mut self, ui: &mut egui::Ui) {
        card(ui, |ui| {
            ui.strong("Atmospheric profile · experimental local refinement");
            muted(
                ui,
                "Temperature, pressure, humidity and wind can vary with altitude. The imported profile replaces fixed sound speed for aircraft and thunder propagation.",
            );
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(!self.busy(), egui::Button::new("Import profile JSON…"))
                    .clicked()
                    && let Some(path) = rfd::FileDialog::new()
                        .add_filter("Atmospheric profile", &["json"])
                        .pick_file()
                    && let Some(previous) = self.document.clone()
                {
                    self.spawn(
                        ui.ctx(),
                        "Checking atmospheric profile and coverage…",
                        move || {
                            geosonnar::atmosphere::load_profile(&path)
                                .map(|profile| ResultValue::Atmosphere(profile, previous))
                        },
                    );
                }
                if ui
                    .add_enabled(
                        !self.busy()
                            && self
                                .document
                                .as_ref()
                                .is_some_and(|d| d.atmosphere.is_some()),
                        egui::Button::new("Remove profile · use fixed sound speed"),
                    )
                    .clicked()
                {
                    self.document.as_mut().unwrap().atmosphere = None;
                    self.score = None;
                }
            });
            let Some(doc) = &self.document else { return };
            let Some(profile) = &doc.atmosphere else {
                ui.label(format!(
                    "Current model: fixed {:.1} m/s. No atmospheric profile loaded.",
                    doc.sound_speed_mps
                ));
                muted(
                    ui,
                    "Import a profile prepared for the recording time and region. The profile is embedded in saved reviews; importing does not change the original file.",
                );
                return;
            };
            ui.strong(&profile.name);
            ui.label(format!(
                "{} · {} altitude levels",
                profile.provenance.provider,
                profile.levels.len()
            ));
            if let (Some(bottom), Some(top)) = (profile.levels.first(), profile.levels.last()) {
                ui.label(format!(
                    "Height coverage: {:.0}–{:.0} m above the WGS84 ellipsoid",
                    bottom.ellipsoid_height_m, top.ellipsoid_height_m
                ));
            }
            ui.label(format!(
                "Bounds S/W/N/E: {:.4}, {:.4}, {:.4}, {:.4}",
                profile.bounds[0], profile.bounds[1], profile.bounds[2], profile.bounds[3]
            ));
            // Relative coverage is easier to check than a bare Unix timestamp.
            if let Ok(start) = workspace::parse_utc(&doc.recording_start_utc) {
                let start = start + doc.map.clock_adjustment_s;
                ui.label(format!(
                    "Coverage relative to adjusted recording start: {:+.1} to {:+.1} s",
                    profile.valid_from_utc_s - start,
                    profile.valid_to_utc_s - start
                ));
                if start < profile.valid_from_utc_s
                    || start + doc.duration_s > profile.valid_to_utc_s
                {
                    ui.colored_label(tone(ui, AMBER), "The profile does not cover the entire recording. Only fully covered source and reception times can be scored.");
                }
            } else {
                ui.label(
                    "Set the recording UTC time in Case to check the profile's time coverage.",
                );
            }
            egui::CollapsingHeader::new("Profile layers and source").show(ui, |ui| {
                ui.label(&profile.provenance.source);
                ui.label(&profile.provenance.notes);
                ui.label(format!(
                    "Source SHA-256: {}",
                    profile.provenance.source_sha256
                ));
                ui.label(format!(
                    "Declared UTC seconds: {:.3}–{:.3}",
                    profile.valid_from_utc_s, profile.valid_to_utc_s
                ));
                egui::ScrollArea::horizontal()
                    .id_salt("atmosphere_layers_scroll")
                    .show(ui, |ui| {
                        egui::Grid::new("atmosphere_layers")
                            .striped(true)
                            .show(ui, |ui| {
                                for title in [
                                    "Height m",
                                    "Temp °C",
                                    "Pressure hPa",
                                    "RH %",
                                    "Wind east m/s",
                                    "Wind north m/s",
                                ] {
                                    ui.strong(title);
                                }
                                ui.end_row();
                                for level in &profile.levels {
                                    for value in [
                                        level.ellipsoid_height_m,
                                        level.temperature_k - 273.15,
                                        level.pressure_pa / 100.0,
                                        level.relative_humidity_fraction * 100.0,
                                        level.eastward_wind_mps,
                                        level.northward_wind_mps,
                                    ] {
                                        ui.label(format!("{value:.2}"));
                                    }
                                    ui.end_row();
                                }
                            });
                    });
            });
            muted(
                ui,
                "Straight-path approximation: no ray bending, terrain obstruction or changing weather. A profile must cover the receiver, selected trajectories and strikes; uncovered paths are unavailable, with no fixed-speed fallback. Local refinement is limited to 400 receivers.",
            );
            muted(
                ui,
                "Weather-provider download and native ERA5/GRIB conversion are not yet included. Profile humidity is relative to liquid water, heights are ellipsoid heights, and winds point toward east/north.",
            );
        });
    }
}
