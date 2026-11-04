//! Small, flushed startup milestones. Never logs case paths, settings or panic payloads.
use std::{
    fs::{File, OpenOptions},
    io::Write,
    path::PathBuf,
    sync::{Mutex, OnceLock},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

const MAX_LOG_BYTES: usize = 64 * 1024;
static SESSION: OnceLock<Option<Mutex<Session>>> = OnceLock::new();

#[derive(Clone, Copy)]
pub enum Stage {
    BeforeGraphics,
    CreationCallback,
    WorkbenchReady,
    FirstUiFrame,
    EventLoopError,
    Exited,
    Panic,
}

impl Stage {
    fn label(self) -> &'static str {
        match self {
            Self::BeforeGraphics => "before_graphics_initialization",
            Self::CreationCallback => "graphics_creation_callback_entered",
            Self::WorkbenchReady => "workbench_initialized",
            Self::FirstUiFrame => "first_ui_frame_completed_before_gpu_presentation",
            Self::EventLoopError => "native_event_loop_error_details_omitted",
            Self::Exited => "native_event_loop_returned",
            Self::Panic => "panic_observed_payload_and_location_omitted",
        }
    }
}

#[derive(Clone, Copy)]
pub enum Backend {
    OpenGl,
    Direct3D12,
    SoftwareDirect3D12,
}

impl Backend {
    fn label(self) -> &'static str {
        match self {
            Self::OpenGl => "selected_backend=opengl",
            Self::Direct3D12 => "selected_backend=direct3d12",
            Self::SoftwareDirect3D12 => "selected_backend=software_direct3d12",
        }
    }
}

struct Session {
    path: PathBuf,
    file: File,
    started: Instant,
    bytes: usize,
    seen: u16,
}

impl Session {
    fn create(directories: &[PathBuf]) -> Option<Self> {
        let epoch = SystemTime::now().duration_since(UNIX_EPOCH).ok()?;
        let name = format!(
            "geosonnar-startup-{}-{}.log",
            std::process::id(),
            epoch.as_nanos()
        );
        for directory in directories {
            let path = directory.join(&name);
            let Ok(file) = OpenOptions::new().write(true).create_new(true).open(&path) else {
                continue;
            };
            let mut session = Self {
                path,
                file,
                started: Instant::now(),
                bytes: 0,
                seen: 0,
            };
            let header = format!(
                "GeoSonnar startup schema=1 version={} os={} arch={} pid={} start_unix_s={}\nNo case paths, arguments, settings values or panic details are recorded.\n",
                env!("CARGO_PKG_VERSION"),
                std::env::consts::OS,
                std::env::consts::ARCH,
                std::process::id(),
                epoch.as_secs(),
            );
            if session.append(&header) {
                return Some(session);
            }
        }
        None
    }

    fn append(&mut self, record: &str) -> bool {
        if record.len() > MAX_LOG_BYTES.saturating_sub(self.bytes) {
            return false;
        }
        // Reserve the bytes even if a write fails partway through.
        self.bytes += record.len();
        self.file.write_all(record.as_bytes()).is_ok()
            && self.file.flush().is_ok()
            && self.file.sync_data().is_ok()
    }

    fn once(&mut self, bit: u16, label: &'static str) {
        if self.seen & bit != 0 {
            return;
        }
        self.seen |= bit;
        let _ = self.append(&format!(
            "elapsed_ms={} {label}\n",
            self.started.elapsed().as_millis()
        ));
    }

    fn stage(&mut self, stage: Stage) {
        self.once(1 << stage as u16, stage.label());
    }

    fn backend(&mut self, backend: Backend) {
        self.once(1 << 8, backend.label());
    }
}

/// Call before initializing any native graphics. Failure to write a log never
/// prevents startup; an unwritable executable directory falls back to temp.
pub fn begin() -> Option<PathBuf> {
    let session = SESSION.get_or_init(|| {
        let mut directories = Vec::new();
        if let Ok(executable) = std::env::current_exe()
            && let Some(parent) = executable.parent()
        {
            directories.push(parent.to_owned());
        }
        directories.push(std::env::temp_dir());
        let result = Session::create(&directories).map(Mutex::new);
        let previous_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            // A panic must never wait on the logging lock or expose its payload here.
            if let Some(Some(session)) = SESSION.get()
                && let Ok(mut session) = session.try_lock()
            {
                session.stage(Stage::Panic);
            }
            previous_hook(info);
        }));
        result
    });
    mark(Stage::BeforeGraphics);
    session
        .as_ref()?
        .lock()
        .ok()
        .map(|session| session.path.clone())
}

pub fn mark(stage: Stage) {
    if let Some(Some(session)) = SESSION.get()
        && let Ok(mut session) = session.lock()
    {
        session.stage(stage);
    }
}

pub fn mark_backend(backend: Backend) {
    if let Some(Some(session)) = SESSION.get()
        && let Ok(mut session) = session.lock()
    {
        session.backend(backend);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    struct Fixture {
        root: PathBuf,
        path: PathBuf,
    }
    impl Fixture {
        fn new() -> Self {
            let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tmp");
            fs::create_dir_all(&root).unwrap();
            let root = root.canonicalize().unwrap();
            let path = root.join(format!(
                "startup-log-test-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir(&path).unwrap();
            Self { root, path }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            if let Ok(path) = self.path.canonicalize() {
                assert!(path.starts_with(&self.root) && path != self.root);
                let _ = fs::remove_dir_all(path);
            }
        }
    }

    #[test]
    fn milestones_are_flushed_once_and_exclude_paths_and_payloads() {
        let fixture = Fixture::new();
        let mut session = Session::create(std::slice::from_ref(&fixture.path)).unwrap();
        for _ in 0..120 {
            session.stage(Stage::BeforeGraphics);
            session.stage(Stage::CreationCallback);
            session.stage(Stage::WorkbenchReady);
            session.stage(Stage::FirstUiFrame);
            session.stage(Stage::Panic);
            session.backend(Backend::OpenGl);
        }
        // Read while the logger is alive, as an interrupted-run diagnostic must allow.
        let text = fs::read_to_string(&session.path).unwrap();
        for stage in [
            Stage::BeforeGraphics,
            Stage::CreationCallback,
            Stage::WorkbenchReady,
            Stage::FirstUiFrame,
            Stage::Panic,
        ] {
            assert_eq!(text.matches(stage.label()).count(), 1);
        }
        assert_eq!(text.matches("selected_backend=").count(), 1);
        assert!(!text.contains(&fixture.path.to_string_lossy().to_string()));
        assert!(text.len() < 2048);
    }

    #[test]
    fn log_is_new_per_run_bounded_and_uses_fallback_directory() {
        let fixture = Fixture::new();
        let not_a_directory = fixture.path.join("blocked");
        fs::write(&not_a_directory, b"preserve me").unwrap();
        let mut first = Session::create(&[not_a_directory.clone(), fixture.path.clone()]).unwrap();
        let second = Session::create(std::slice::from_ref(&fixture.path)).unwrap();
        assert_ne!(first.path, second.path);
        assert_eq!(first.path.parent(), Some(fixture.path.as_path()));
        assert_eq!(fs::read(&not_a_directory).unwrap(), b"preserve me");
        let remaining = MAX_LOG_BYTES - first.bytes;
        assert!(first.append(&"x".repeat(remaining)));
        assert!(!first.append("overflow"));
        assert_eq!(
            fs::metadata(&first.path).unwrap().len(),
            MAX_LOG_BYTES as u64
        );
    }
}
