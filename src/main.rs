mod win32;
mod github;

use eframe::egui;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use tracing::warn;
use tracing_subscriber::EnvFilter;
use windows::Win32::System::Threading::WaitForSingleObject;

#[derive(PartialEq, Clone, Copy)]
enum GameVersion {
    Global,
    Japanese,
}

#[derive(PartialEq, Clone, Copy)]
pub enum HachimiVersion {
    Original,
    Edge,
}

struct TamamoApp {
    hachimi_enabled: bool,
    hachimi_edge_enabled: bool,
    game_version: GameVersion,
    custom_dlls: Vec<PathBuf>,
    auto_restart: bool,
    is_watching: bool,
    status: String,
    state: Arc<Mutex<AppState>>,
}

struct AppState {
    is_watching: bool,
    status: String,
    should_stop: bool,
}

impl TamamoApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // Install image loaders for egui
        egui_extras::install_image_loaders(&cc.egui_ctx);

        Self {
            hachimi_enabled: true,
            hachimi_edge_enabled: false,
            game_version: GameVersion::Global,
            custom_dlls: Vec::new(),
            auto_restart: true,
            is_watching: false,
            status: "Idle".to_string(),
            state: Arc::new(Mutex::new(AppState {
                is_watching: false,
                status: "Idle".to_string(),
                should_stop: false,
            })),
        }
    }

    fn start_watching(&mut self) {
        let state = self.state.clone();
        let hachimi = self.hachimi_enabled;
        let hachimi_edge = self.hachimi_edge_enabled;
        let game_version = self.game_version;
        let custom = self.custom_dlls.clone();
        let auto_restart = self.auto_restart;

        {
            let mut s = state.lock().unwrap();
            s.is_watching = true;
            s.should_stop = false;
            s.status = "Watching for Umamusume...".to_string();
        }
        self.is_watching = true;

        thread::spawn(move || {
            let mut downloaded_dlls = Vec::new();

            if hachimi {
                {
                    let mut s = state.lock().unwrap();
                    s.status = "Downloading latest Hachimi...".to_string();
                }
                match github::hachimi_download_latest(HachimiVersion::Original) {
                    Ok(path) => downloaded_dlls.push(path),
                    Err(e) => {
                        let mut s = state.lock().unwrap();
                        s.status = format!("Download failed: {}", e);

                        rfd::MessageDialog::new()
                            .set_title("Download Error")
                            .set_description(format!(
                                "Failed to download Hachimi: {}\n\nInjection will continue without this DLL.",
                                e
                            ))
                            .set_level(rfd::MessageLevel::Error)
                            .show();
                    }
                }
            }
            if hachimi_edge {
                {
                    let mut s = state.lock().unwrap();
                    s.status = "Downloading latest Hachimi Edge...".to_string();
                }
                match github::hachimi_download_latest(HachimiVersion::Edge) {
                    Ok(path) => downloaded_dlls.push(path),
                    Err(e) => {
                        let mut s = state.lock().unwrap();
                        s.status = format!("Download failed: {}", e);

                        rfd::MessageDialog::new()
                            .set_title("Download Error")
                            .set_description(format!(
                                "Failed to download Hachimi Edge: {}\n\nInjection will continue without this DLL.",
                                e
                            ))
                            .set_level(rfd::MessageLevel::Error)
                            .show();
                    }
                }
            }

            let process_name = match game_version {
                GameVersion::Global => "UmamusumePrettyDerby.exe",
                GameVersion::Japanese => "umamusume.exe",
            };

            loop {
                {
                    let mut s = state.lock().unwrap();
                    s.status = format!("Watching for {}...", process_name);
                }

                // 1. Wait for process
                let ph = loop {
                    {
                        let s = state.lock().unwrap();
                        if s.should_stop {
                            return;
                        }
                    }
                    if let Some(ph) = win32::find_process(process_name) {
                        break ph;
                    }
                    thread::sleep(std::time::Duration::from_millis(500));
                };

                {
                    let mut s = state.lock().unwrap();
                    s.status = "Process found! Waiting for window...".to_string();
                }

                // 2. Wait for window
                loop {
                    {
                        let s = state.lock().unwrap();
                        if s.should_stop {
                            return;
                        }
                    }
                    if win32::has_window(ph) {
                        break;
                    }
                    thread::sleep(std::time::Duration::from_millis(500));
                }

                {
                    let mut s = state.lock().unwrap();
                    s.status = "Waiting for process to become idle...".to_string();
                }
                win32::wait_for_input_idle(ph, 10000);

                thread::sleep(std::time::Duration::from_millis(1000));

                let mut dlls_to_inject = downloaded_dlls.clone();
                for d in &custom {
                    if let Some(s) = d.to_str() {
                        dlls_to_inject.push(s.to_string());
                    }
                }


                let mut success_count = 0;
                for dll in &dlls_to_inject {
                    let dll_path = Path::new(dll);
                    let absolute_dll_path = if dll_path.is_relative() {
                        std::env::current_dir().unwrap().join(dll_path)
                    } else {
                        dll_path.to_path_buf()
                    };

                    if let Some(path_str) = absolute_dll_path.to_str() {
                        if unsafe { win32::inject_dll_to_handle(ph, path_str) } {
                            success_count += 1;
                        }
                    }
                }

                {
                    let mut s = state.lock().unwrap();
                    s.status = format!("Injected {}/{} DLLs", success_count, dlls_to_inject.len());
                }

                if !auto_restart {
                    let mut s = state.lock().unwrap();
                    s.is_watching = false;
                    return;
                }

                {
                    let mut s = state.lock().unwrap();
                    s.status = "Injected. Waiting for process to exit...".to_string();
                }

                // 3. Wait for process to exit
                loop {
                    {
                        let s = state.lock().unwrap();
                        if s.should_stop {
                            return;
                        }
                    }
                    unsafe {
                        let wait_result = WaitForSingleObject(ph, 500);
                        if wait_result == windows::Win32::Foundation::WAIT_OBJECT_0 {
                            break;
                        }
                    }
                }

                {
                    let mut s = state.lock().unwrap();
                    s.status = "Process exited. Restarting watch...".to_string();
                }
            }
        });
    }

    fn stop_watching(&mut self) {
        let mut s = self.state.lock().unwrap();
        s.should_stop = true;
        s.is_watching = false;
        s.status = "Stopping...".to_string();
        self.is_watching = false;
    }
}

impl eframe::App for TamamoApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Sync state
        {
            let s = self.state.lock().unwrap();
            self.is_watching = s.is_watching;
            self.status = s.status.clone();
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.add(
                    egui::Image::new(egui::include_image!("../assets/tamamo-x.png"))
                        .max_width(200.0)
                        .corner_radius(10.0),
                );
                ui.heading("Tamamo-X");
            });

            ui.separator();

            ui.vertical(|ui| {
                ui.label("Game Version:");
                ui.radio_value(&mut self.game_version, GameVersion::Global, "Global (UmamusumePrettyDerby.exe)");
                ui.radio_value(&mut self.game_version, GameVersion::Japanese, "Japanese (umamusume.exe)");
                
                ui.separator();

                ui.label("Injection Options:");
                if ui.checkbox(&mut self.hachimi_enabled, "Inject Hachimi").clicked() && self.hachimi_enabled {
                    self.hachimi_edge_enabled = false;
                }
                if ui.checkbox(&mut self.hachimi_edge_enabled, "Inject Hachimi-Edge").clicked() && self.hachimi_edge_enabled {
                    self.hachimi_enabled = false;
                }

                ui.group(|ui| {
                    ui.label("Custom DLLs:");
                    let mut to_remove = None;
                    for (i, dll) in self.custom_dlls.iter().enumerate() {
                        ui.horizontal(|ui| {
                            ui.label(dll.file_name().unwrap_or_default().to_string_lossy());
                            if ui.button("❌").clicked() {
                                to_remove = Some(i);
                            }
                        });
                    }
                    if let Some(i) = to_remove {
                        self.custom_dlls.remove(i);
                    }

                    if ui.button("Add Custom DLL...").clicked() {
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter("DLL Files", &["dll"])
                            .pick_file()
                        {
                            self.custom_dlls.push(path);
                        }
                    }
                });

                ui.separator();

                ui.checkbox(
                    &mut self.auto_restart,
                    "Auto-restart watching when game stops",
                );

                ui.horizontal(|ui| {
                    if self.is_watching {
                        if ui.button("Stop Watching").clicked() {
                            self.stop_watching();
                        }
                    } else {
                        if ui.button("Start Watching").clicked() {
                            self.start_watching();
                        }
                    }
                });

                ui.separator();
                ui.horizontal(|ui| {
                    ui.label("Status:");
                    ui.label(&self.status);
                });
            });
        });

        // Request repaint to keep status updated
        if self.is_watching {
            ctx.request_repaint();
        }
    }
}

fn main() -> eframe::Result {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive(tracing::Level::INFO.into()))
        .init();

    warn!(
        "Tamamo-X v{} | GitHub: https://github.com/teppyboy/tamamo-x",
        env!("CARGO_PKG_VERSION")
    );
    warn!(
        "This is an experimental software, hence I will NOT be responsible for any damage. Use at your own risk."
    );

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([400.0, 600.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };

    eframe::run_native(
        "Tamamo-X",
        options,
        Box::new(|cc| Ok(Box::new(TamamoApp::new(cc)))),
    )
}
