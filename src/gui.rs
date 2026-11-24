use crate::actions::{self, DesktopEnv};
use crate::config::{Action, AppConfig, Corner};
use crate::service;
use anyhow::Result;
use eframe::egui::{self, Align, Color32, CornerRadius, Frame, Layout, RichText, Stroke, Vec2};
use std::time::{Duration, Instant};

struct HotCornersApp {
    config: AppConfig,
    saved_config: AppConfig,
    daemon_running: bool,
    autostart_enabled: bool,
    desktop_env: DesktopEnv,
    last_status_poll: Instant,
    status_message: Option<(String, Instant)>,
}

impl HotCornersApp {
    fn new() -> Self {
        let config = AppConfig::load().unwrap_or_default();
        let saved_config = config.clone();
        Self {
            config,
            saved_config,
            daemon_running: service::is_daemon_running(),
            autostart_enabled: service::is_autostart_enabled(),
            desktop_env: DesktopEnv::detect(),
            last_status_poll: Instant::now(),
            status_message: None,
        }
    }

    fn set_status(&mut self, msg: impl Into<String>) {
        self.status_message = Some((msg.into(), Instant::now()));
    }

    fn save_if_dirty(&mut self) {
        if self.config != self.saved_config {
            match self.config.save() {
                Ok(_) => {
                    self.saved_config = self.config.clone();
                    if self.daemon_running {
                        self.set_status("Saved — daemon will live-reload");
                    } else {
                        self.set_status("Saved to config.toml");
                    }
                }
                Err(err) => {
                    self.set_status(format!("Save failed: {err}"));
                }
            }
        }
    }

    fn corner_picker(&mut self, ui: &mut egui::Ui, corner: Corner) {
        let presets = Action::presets();
        let current_action = self.config.action_for(corner).clone();

        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(corner.label()).strong());
                if current_action.is_enabled() && ui.small_button("Test").clicked() {
                    if let Err(err) = actions::execute_action(&current_action) {
                        self.set_status(format!("Test failed: {err}"));
                    } else {
                        self.set_status(format!("Triggered {}", corner.label()));
                    }
                }
            });

            let action = self.config.action_for_mut(corner);
            egui::ComboBox::from_id_salt(corner.slug())
                .selected_text(action.label())
                .width(210.0)
                .show_ui(ui, |ui| {
                    for preset in &presets {
                        let selected = action.is_same_variant(preset);
                        if ui.selectable_label(selected, preset.label()).clicked() && !selected {
                            *action = preset.clone();
                        }
                    }
                });

            if let Action::Custom { command } = self.config.action_for_mut(corner) {
                ui.add(
                    egui::TextEdit::singleline(command)
                        .desired_width(210.0)
                        .hint_text("Shell command, e.g. cosmic-term"),
                );
            }
        });
    }

    fn draw_monitor_preview(&self, ui: &mut egui::Ui) {
        let desired_size = Vec2::new(260.0, 148.0);
        let (rect, _response) = ui.allocate_exact_size(desired_size, egui::Sense::hover());
        let painter = ui.painter();

        // Monitor bezel
        let screen_rect = rect.shrink2(Vec2::new(10.0, 6.0));
        painter.rect(
            screen_rect,
            CornerRadius::same(10),
            Color32::from_rgb(24, 28, 36),
            Stroke::new(2.0_f32, Color32::from_rgb(72, 82, 98)),
            egui::StrokeKind::Middle,
        );

        // Subtle inner desktop wallpaper gradient hint
        let inner = screen_rect.shrink(6.0);
        painter.rect_filled(inner, CornerRadius::same(6), Color32::from_rgb(33, 40, 54));

        // Monitor stand
        let stand_w = 52.0;
        let stand_rect = egui::Rect::from_center_size(
            egui::pos2(rect.center().x, screen_rect.bottom() + 6.0),
            Vec2::new(stand_w, 6.0),
        );
        painter.rect_filled(stand_rect, CornerRadius::same(3), Color32::from_rgb(72, 82, 98));

        // Corner hot-zone indicators
        let active_color = Color32::from_rgb(70, 165, 255);
        let inactive_color = Color32::from_rgb(65, 74, 90);
        let dot_r = 7.0;
        let inset = 11.0;

        let corners = [
            (Corner::TopLeft, egui::pos2(inner.left() + inset, inner.top() + inset)),
            (Corner::TopRight, egui::pos2(inner.right() - inset, inner.top() + inset)),
            (Corner::BottomLeft, egui::pos2(inner.left() + inset, inner.bottom() - inset)),
            (Corner::BottomRight, egui::pos2(inner.right() - inset, inner.bottom() - inset)),
        ];

        for (corner, pos) in corners {
            let enabled = self.config.action_for(corner).is_enabled();
            let color = if enabled { active_color } else { inactive_color };
            if enabled {
                painter.circle_filled(pos, dot_r + 4.0, Color32::from_rgba_unmultiplied(70, 165, 255, 45));
            }
            painter.circle_filled(pos, dot_r, color);
        }

        // Center label
        painter.text(
            inner.center(),
            egui::Align2::CENTER_CENTER,
            format!("{} Display", self.desktop_env.name()),
            egui::FontId::proportional(13.0),
            Color32::from_rgb(160, 174, 196),
        );
    }
}

impl eframe::App for HotCornersApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.last_status_poll.elapsed() >= Duration::from_secs(1) {
            self.daemon_running = service::is_daemon_running();
            self.last_status_poll = Instant::now();
        }
        ctx.request_repaint_after(Duration::from_secs(1));

        egui::TopBottomPanel::top("header")
            .frame(Frame::new().inner_margin(egui::Margin::symmetric(16, 12)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.heading("Open Corners");
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new(format!("({})", self.desktop_env.name()))
                            .weak()
                            .size(13.0),
                    );

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if self.daemon_running {
                            if ui.button("Stop Daemon").clicked() {
                                match service::stop_daemon() {
                                    Ok(()) => {
                                        self.daemon_running = false;
                                        self.set_status("Daemon stopped");
                                    }
                                    Err(err) => self.set_status(format!("Stop failed: {err}")),
                                }
                            }
                            ui.label(
                                RichText::new("● Active")
                                    .color(Color32::from_rgb(80, 210, 120))
                                    .strong(),
                            );
                        } else {
                            if ui.button("Start Daemon").clicked() {
                                self.save_if_dirty();
                                match service::start_daemon() {
                                    Ok(()) => {
                                        self.daemon_running = true;
                                        self.set_status("Daemon started");
                                    }
                                    Err(err) => self.set_status(format!("Start failed: {err}")),
                                }
                            }
                            ui.label(
                                RichText::new("○ Stopped")
                                    .color(Color32::from_rgb(180, 180, 190)),
                            );
                        }
                    });
                });
            });

        egui::TopBottomPanel::bottom("footer")
            .frame(Frame::new().inner_margin(egui::Margin::symmetric(16, 10)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let mut autostart = self.autostart_enabled;
                    if ui
                        .checkbox(&mut autostart, "Start daemon automatically on login")
                        .changed()
                    {
                        match service::set_autostart(autostart) {
                            Ok(()) => {
                                self.autostart_enabled = autostart;
                                self.daemon_running = service::is_daemon_running();
                                self.set_status(if autostart {
                                    "Autostart enabled (systemd user service)"
                                } else {
                                    "Autostart disabled"
                                });
                            }
                            Err(err) => self.set_status(format!("Autostart error: {err}")),
                        }
                    }

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.button("Install App Entry").clicked() {
                            match service::install_desktop_entry() {
                                Ok(path) => {
                                    self.set_status(format!("Installed {}", path.display()))
                                }
                                Err(err) => self.set_status(format!("Install failed: {err}")),
                            }
                        }

                        if let Some((msg, at)) = &self.status_message {
                            if at.elapsed() < Duration::from_secs(5) {
                                ui.label(RichText::new(msg).weak().size(12.0));
                            }
                        }
                    });
                });
            });

        egui::CentralPanel::default()
            .frame(Frame::new().inner_margin(egui::Margin::symmetric(18, 10)))
            .show(ctx, |ui| {
                // Top row: Top-Left & Top-Right
                ui.columns(2, |cols| {
                    cols[0].with_layout(Layout::top_down(Align::LEFT), |ui| {
                        self.corner_picker(ui, Corner::TopLeft);
                    });
                    cols[1].with_layout(Layout::top_down(Align::RIGHT), |ui| {
                        self.corner_picker(ui, Corner::TopRight);
                    });
                });

                ui.add_space(10.0);

                // Center: Monitor diagram
                ui.vertical_centered(|ui| {
                    self.draw_monitor_preview(ui);
                });

                ui.add_space(10.0);

                // Bottom row: Bottom-Left & Bottom-Right
                ui.columns(2, |cols| {
                    cols[0].with_layout(Layout::top_down(Align::LEFT), |ui| {
                        self.corner_picker(ui, Corner::BottomLeft);
                    });
                    cols[1].with_layout(Layout::top_down(Align::RIGHT), |ui| {
                        self.corner_picker(ui, Corner::BottomRight);
                    });
                });

                ui.add_space(14.0);
                ui.separator();
                ui.add_space(8.0);

                ui.label(RichText::new("Sensitivity & Behavior").strong());
                ui.add_space(4.0);

                egui::Grid::new("behavior_grid")
                    .num_columns(2)
                    .spacing([16.0, 6.0])
                    .show(ui, |ui| {
                        ui.label("Dwell delay:");
                        ui.add(
                            egui::Slider::new(&mut self.config.dwell_ms, 0..=600)
                                .suffix(" ms"),
                        );
                        ui.end_row();

                        ui.label("Trigger cooldown:");
                        ui.add(
                            egui::Slider::new(&mut self.config.cooldown_ms, 100..=2000)
                                .suffix(" ms"),
                        );
                        ui.end_row();

                        ui.label("Corner hit-zone size:");
                        ui.add(
                            egui::Slider::new(&mut self.config.corner_size_px, 2..=32)
                                .suffix(" px"),
                        );
                        ui.end_row();
                    });

                ui.add_space(4.0);
                ui.checkbox(
                    &mut self.config.debug_overlay,
                    "Show translucent debug markers in active corners",
                );
            });

        self.save_if_dirty();
    }
}

pub fn run_gui() -> Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Open Corners")
            .with_inner_size([560.0, 520.0])
            .with_min_inner_size([500.0, 470.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Open Corners",
        options,
        Box::new(|cc| {
            cc.egui_ctx.set_visuals(egui::Visuals::dark());
            Ok(Box::new(HotCornersApp::new()))
        }),
    )
    .map_err(|e| anyhow::anyhow!("Failed to launch Settings GUI: {e}"))
}
