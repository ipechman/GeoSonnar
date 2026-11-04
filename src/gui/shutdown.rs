//! Confirmed, bounded application exit and Windows ownership of helper processes.
use std::time::{Duration, Instant};

pub(super) const EXIT_GRACE: Duration = Duration::from_secs(2);

#[derive(Default)]
pub(super) struct ExitState {
    requested_at: Option<Instant>,
}

impl ExitState {
    /// Repeated close requests must never extend the deadline.
    pub(super) fn request(&mut self, now: Instant) -> bool {
        if self.requested_at.is_some() {
            return false;
        }
        self.requested_at = Some(now);
        true
    }

    pub(super) fn requested(&self) -> bool {
        self.requested_at.is_some()
    }

    pub(super) fn ready(&self, now: Instant) -> bool {
        self.requested_at
            .is_some_and(|started| now.saturating_duration_since(started) >= EXIT_GRACE)
    }
}

impl super::Workbench {
    pub(super) fn begin_cancel_and_exit(&mut self, ctx: &eframe::egui::Context) {
        if !self.shutdown.request(Instant::now()) {
            return;
        }
        self.close_requested = true;
        self.studio.cancel_for_exit();
        // Request the saved job's normal cancellation first. This operation may
        // itself wait on a CLI, so it must never block the UI or exit deadline.
        // Only the job this GUI launched is selected, not unrelated runners.
        if let (Some(job), Some(case), Ok(companion)) = (&self.running, &self.case, &self.companion)
        {
            let job = job.clone();
            let manifest = case.manifest.clone();
            let companion = companion.clone();
            let _ = std::thread::Builder::new()
                .name("cancel-job-on-exit".into())
                .spawn(move || {
                    let _ = companion.cancel_job(&manifest, &job);
                });
        }
        ctx.request_repaint();
    }
}

/// Install before any GUI worker can launch helpers. The unnamed, noninheritable
/// handle stays open until OS process teardown, which terminates only this GUI's
/// descendants. Never install this in the main test process.
pub fn install_process_guard() -> Result<(), String> {
    #[cfg(windows)]
    {
        windows::install()
    }
    #[cfg(not(windows))]
    {
        Err("Safe cancellation of the GUI's helper processes is currently supported only on Windows".into())
    }
}

#[cfg(windows)]
mod windows {
    use std::{ffi::c_void, io};

    #[repr(C)]
    #[derive(Default)]
    struct BasicLimits {
        per_process_user_time: i64,
        per_job_user_time: i64,
        flags: u32,
        minimum_working_set: usize,
        maximum_working_set: usize,
        active_process_limit: u32,
        affinity: usize,
        priority_class: u32,
        scheduling_class: u32,
    }

    #[repr(C)]
    #[derive(Default)]
    struct ExtendedLimits {
        basic: BasicLimits,
        io_counters: [u64; 6],
        process_memory: usize,
        job_memory: usize,
        peak_process_memory: usize,
        peak_job_memory: usize,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateJobObjectW(attributes: *const c_void, name: *const u16) -> *mut c_void;
        fn SetInformationJobObject(
            job: *mut c_void,
            class: i32,
            info: *const c_void,
            size: u32,
        ) -> i32;
        fn AssignProcessToJobObject(job: *mut c_void, process: *mut c_void) -> i32;
        fn GetCurrentProcess() -> *mut c_void;
        fn CloseHandle(handle: *mut c_void) -> i32;
    }

    struct UnassignedJob(*mut c_void);
    impl Drop for UnassignedJob {
        fn drop(&mut self) {
            // Only used on setup failure, before this process joins the job.
            unsafe {
                CloseHandle(self.0);
            }
        }
    }

    pub(super) fn install() -> Result<(), String> {
        // CreateJobObjectW with null security attributes makes the returned
        // handle noninheritable. No breakaway flags and no parent PID lookup.
        // https://learn.microsoft.com/windows/win32/procthread/job-objects
        unsafe {
            let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if handle.is_null() {
                return Err(format!(
                    "Cannot create helper-process ownership: {}",
                    io::Error::last_os_error()
                ));
            }
            let guard = UnassignedJob(handle);
            let limits = ExtendedLimits {
                basic: BasicLimits {
                    flags: 0x2000, // JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
                    ..Default::default()
                },
                ..Default::default()
            };
            if SetInformationJobObject(
                handle,
                9, // JobObjectExtendedLimitInformation
                (&limits as *const ExtendedLimits).cast(),
                std::mem::size_of::<ExtendedLimits>() as u32,
            ) == 0
            {
                return Err(format!(
                    "Cannot configure helper-process cleanup: {}",
                    io::Error::last_os_error()
                ));
            }
            if AssignProcessToJobObject(handle, GetCurrentProcess()) == 0 {
                return Err(format!(
                    "Cannot establish helper-process cleanup: {}",
                    io::Error::last_os_error()
                ));
            }
            // Closing now would terminate the GUI itself. The OS closes this
            // sole handle on every process-exit path, including forced exits.
            std::mem::forget(guard);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_deadline_does_not_depend_on_busy_state_or_extend_on_repeat() {
        let start = Instant::now();
        let mut exit = ExitState::default();
        assert!(!exit.requested() && !exit.ready(start + Duration::from_secs(20)));
        assert!(exit.request(start));
        assert!(exit.requested());
        assert!(!exit.ready(start + EXIT_GRACE - Duration::from_millis(1)));
        assert!(!exit.request(start + Duration::from_secs(1)));
        assert!(exit.ready(start + EXIT_GRACE));
        assert!(exit.ready(start + Duration::from_secs(20)));
    }

    fn empty_workbench() -> (super::super::Workbench, eframe::egui::Context) {
        let ctx = eframe::egui::Context::default();
        let creation = eframe::CreationContext::_new_kittest(ctx.clone());
        let unused_executable = std::env::temp_dir()
            .join(format!(
                "geosonnar-no-shutdown-settings-{}",
                std::process::id()
            ))
            .join("geosonnar-gui.exe");
        (
            super::super::Workbench::with_executable(&creation, unused_executable, None),
            ctx,
        )
    }

    #[test]
    fn confirmed_exit_closes_despite_stuck_worker_and_missing_companion_without_saving() {
        let (mut app, ctx) = empty_workbench();
        app.busy = Some("A noncancelable read is still busy".into());
        app.running = Some("fabricated-running-job".into());
        app.canceling = true;
        app.companion = Err("Companion unavailable".into());
        app.begin_cancel_and_exit(&ctx);
        assert!(app.shutdown.requested());
        assert!(!app.allow_close);
        app.shutdown.requested_at = Some(Instant::now() - EXIT_GRACE);
        app.closing(&ctx);
        assert!(app.allow_close);
        assert!(app.busy.is_some() && app.running.is_some() && app.canceling);
        assert!(
            !app.preferences_path.exists(),
            "Exit must not force-save settings or evidence"
        );
    }

    #[test]
    fn shutdown_ignores_late_case_and_error_messages_without_starting_followup_work() {
        use super::super::{Lane, Message, Payload};
        use geosonnar::{
            casework::{CaseMedia, CaseRecording, SavedCase},
            gui_bridge::CaseOverview,
        };
        let (mut app, ctx) = empty_workbench();
        app.begin_cancel_and_exit(&ctx);
        let case = CaseOverview {
            manifest: "never-open-this-case.json".into(),
            case: SavedCase {
                schema_version: 1,
                case_id: "synthetic-exit".into(),
                title: "Late result must be ignored".into(),
                created_unix_s: 1,
                local_only: true,
                recording: CaseRecording {
                    path: "unused.raw".into(),
                    bytes: 0,
                    sha256: "0".repeat(64),
                    media: CaseMedia {
                        duration_s: 30.0,
                        sample_rate_hz: 8000,
                        channels: 1,
                        audio_codec: "synthetic".into(),
                    },
                },
            },
            hints: Vec::new(),
            recording_path: "unused.raw".into(),
            recording_verified: Some(true),
            recording_problem: None,
        };
        for result in [
            Ok(Payload::Case(case)),
            Err("Late operation failure".into()),
        ] {
            app.sender
                .send(Message {
                    lane: Lane::Work,
                    case_manifest: None,
                    result,
                })
                .unwrap();
        }
        app.update_messages(&ctx);
        assert!(app.case.is_none() && app.notice.is_none() && !app.polling);
        assert!(app.shutdown.requested());
        assert!(app.receiver.try_recv().is_err());
        app.studio.assert_unloaded_idle();
        assert!(!app.preferences_path.exists());
    }

    #[cfg(windows)]
    mod process_ownership {
        use super::*;
        use std::{
            ffi::c_void,
            fs,
            os::windows::process::CommandExt,
            path::{Path, PathBuf},
            process::{Child, Command, Stdio},
            thread,
        };

        const PROBE_TEST: &str = "gui::shutdown::tests::process_ownership::isolated_process_probe";
        const MODE: &str = "GEOSONNAR_SHUTDOWN_TEST_MODE";
        const DIRECTORY: &str = "GEOSONNAR_SHUTDOWN_TEST_DIRECTORY";

        struct ChildCleanup(Child);
        impl Drop for ChildCleanup {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }

        struct DirectoryCleanup(PathBuf);
        impl Drop for DirectoryCleanup {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }

        fn probe(mode: &str, directory: &Path) -> ChildCleanup {
            ChildCleanup(
                Command::new(std::env::current_exe().unwrap())
                    .args(["--exact", PROBE_TEST, "--ignored", "--nocapture"])
                    .env(MODE, mode)
                    .env(DIRECTORY, directory)
                    .creation_flags(0x0800_0000)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .unwrap(),
            )
        }

        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn OpenProcess(access: u32, inherit: i32, process_id: u32) -> *mut c_void;
            fn WaitForSingleObject(handle: *mut c_void, milliseconds: u32) -> u32;
            fn CloseHandle(handle: *mut c_void) -> i32;
        }

        struct ObservedProcess(*mut c_void);
        impl Drop for ObservedProcess {
            fn drop(&mut self) {
                unsafe {
                    CloseHandle(self.0);
                }
            }
        }

        #[test]
        fn gui_exit_kills_owned_descendant_but_leaves_sibling_and_test_parent_alive() {
            let directory = DirectoryCleanup(std::env::temp_dir().join(format!(
                    "geosonnar-shutdown-{}-{}",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_nanos()
                )));
            fs::create_dir(&directory.0).unwrap();
            let mut sibling = probe("wait", &directory.0);
            let mut owner = probe("owner", &directory.0);
            let deadline = Instant::now() + Duration::from_secs(10);
            let descendant_id = loop {
                if let Ok(text) = fs::read_to_string(directory.0.join("descendant.pid"))
                    && let Ok(id) = text.parse::<u32>()
                {
                    break id;
                }
                assert!(
                    owner.0.try_wait().unwrap().is_none(),
                    "Isolated ownership probe exited before setup"
                );
                assert!(
                    Instant::now() < deadline,
                    "Ownership probe did not report its descendant"
                );
                thread::sleep(Duration::from_millis(20));
            };
            // Open while the owner is still waiting for our handoff, so this is
            // the exact owned descendant, never a stale PID used for killing.
            let descendant = ObservedProcess(unsafe { OpenProcess(0x0010_0000, 0, descendant_id) });
            assert!(!descendant.0.is_null());
            assert_eq!(unsafe { WaitForSingleObject(descendant.0, 0) }, 258);
            fs::write(directory.0.join("exit.request"), []).unwrap();
            loop {
                if let Some(status) = owner.0.try_wait().unwrap() {
                    assert!(status.success());
                    break;
                }
                assert!(Instant::now() < deadline, "Ownership probe did not exit");
                thread::sleep(Duration::from_millis(20));
            }
            assert_eq!(
                unsafe { WaitForSingleObject(descendant.0, 5000) },
                0,
                "An owned descendant survived its GUI owner"
            );
            assert!(
                sibling.0.try_wait().unwrap().is_none(),
                "Unrelated sibling was terminated"
            );
        }

        /// Executed only in a disposable child by the preceding test. A normal
        /// --ignored test run without the private mode performs no assignment.
        #[test]
        #[ignore]
        fn isolated_process_probe() {
            let Ok(mode) = std::env::var(MODE) else {
                return;
            };
            let directory = PathBuf::from(std::env::var_os(DIRECTORY).unwrap());
            if mode == "wait" {
                thread::sleep(Duration::from_secs(30));
                return;
            }
            assert_eq!(mode, "owner");
            install_process_guard().unwrap();
            let descendant = probe("wait", &directory);
            fs::write(
                directory.join("descendant.pid"),
                descendant.0.id().to_string(),
            )
            .unwrap();
            let deadline = Instant::now() + Duration::from_secs(10);
            while !directory.join("exit.request").exists() {
                assert!(
                    Instant::now() < deadline,
                    "Parent did not complete ownership probe"
                );
                thread::sleep(Duration::from_millis(20));
            }
            // Deliberately skip Rust cleanup: only OS closure of the job handle
            // can stop the descendant on this successful test path.
            std::process::exit(0);
        }
    }
}
