//! Offline helper-discovery controls. Fixtures are text files, never programs.
use geosonnar::media_tools::{
    MAX_PATH_ENTRIES, Tool, resolve, resolve_with_environment, resolve_with_program_data,
};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let parent = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tmp");
        fs::create_dir_all(&parent).unwrap();
        loop {
            let root = parent.join(format!(
                "media-tools-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            match fs::create_dir(&root) {
                Ok(()) => return Self(root.canonicalize().unwrap()),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("Create helper fixture: {error}"),
            }
        }
    }

    fn directory(&self, name: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn helper(&self, directory: &Path, tool: Tool) -> PathBuf {
        let path = directory.join(tool.filename());
        fs::write(
            &path,
            b"Metadata-only fixture: not an executable; never launch.",
        )
        .unwrap();
        path.canonicalize().unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let parent = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tmp")
            .canonicalize()
            .unwrap();
        assert_eq!(self.0.parent(), Some(parent.as_path()));
        assert!(
            !fs::symlink_metadata(&self.0)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn bundled_then_beside_program_then_ordered_absolute_path_for_every_tool() {
    let fixture = Fixture::new();
    let program = fixture.directory("portable app");
    let bundled = fixture.directory("portable app/tools");
    let first = fixture.directory("first PATH folder");
    let second = fixture.directory("second PATH folder");
    let search_path = std::env::join_paths([&first, &second]).unwrap();
    for tool in [Tool::Ffmpeg, Tool::Ffprobe, Tool::Ffplay] {
        let candidates = [
            fixture.helper(&bundled, tool),
            fixture.helper(&program, tool),
            fixture.helper(&first, tool),
            fixture.helper(&second, tool),
        ];
        for expected in candidates {
            let actual =
                resolve_with_environment(tool, None, &fixture.0, &program, Some(&search_path))
                    .unwrap();
            assert_eq!(actual, expected);
            fs::remove_file(expected).unwrap();
        }
        assert!(
            resolve_with_environment(tool, None, &fixture.0, &program, Some(&search_path)).is_err()
        );
    }
}

#[test]
fn explicit_paths_keep_their_base_and_never_fall_back_when_invalid() {
    let fixture = Fixture::new();
    let program = fixture.directory("program");
    let config = fixture.directory("config folder");
    let selected = fixture.helper(&config, Tool::Ffprobe);
    fixture.helper(&program, Tool::Ffprobe);
    let automatic_path = std::env::join_paths([&program]).unwrap();
    let choose = |path: &Path| {
        resolve_with_environment(
            Tool::Ffprobe,
            Some(path),
            &config,
            &program,
            Some(&automatic_path),
        )
    };
    assert_eq!(
        choose(Path::new(Tool::Ffprobe.filename())).unwrap(),
        selected
    );
    assert_eq!(
        resolve(Tool::Ffprobe, Some(&selected), Path::new("")).unwrap(),
        selected
    );
    for missing in [
        config.join("missing.exe"),
        PathBuf::from("missing.exe"),
        config.clone(),
    ] {
        let error = choose(&missing).unwrap_err();
        assert!(error.contains("explicit selection was not replaced"));
    }
    assert!(choose(Path::new("")).unwrap_err().contains("explicit"));
    assert!(
        resolve(
            Tool::Ffprobe,
            Some(Path::new("relative.exe")),
            Path::new("relative base")
        )
        .is_err()
    );
}

#[test]
fn directory_named_like_a_helper_does_not_mask_later_path_files() {
    let fixture = Fixture::new();
    let program = fixture.directory("program");
    let first = fixture.directory("first");
    let second = fixture.directory("second");
    fs::create_dir(first.join(Tool::Ffmpeg.filename())).unwrap();
    let expected = fixture.helper(&second, Tool::Ffmpeg);
    let search_path = std::env::join_paths([&first, &second]).unwrap();
    assert_eq!(
        resolve_with_environment(Tool::Ffmpeg, None, &fixture.0, &program, Some(&search_path))
            .unwrap(),
        expected
    );
}

#[test]
fn automatic_discovery_skips_empty_relative_entries_and_has_a_fixed_scan_limit() {
    let fixture = Fixture::new();
    let program = fixture.directory("program");
    let local = fixture.directory("actual tools");
    let expected = fixture.helper(&local, Tool::Ffprobe);
    let search_path = std::env::join_paths([Path::new(""), Path::new("relative"), &local]).unwrap();
    assert_eq!(
        resolve_with_environment(
            Tool::Ffprobe,
            None,
            &fixture.0,
            &program,
            Some(&search_path)
        )
        .unwrap(),
        expected
    );
    let entries =
        std::iter::repeat_n(Path::new("relative"), MAX_PATH_ENTRIES).chain([local.as_path()]);
    let capped = std::env::join_paths(entries).unwrap();
    assert!(
        resolve_with_environment(Tool::Ffprobe, None, &fixture.0, &program, Some(&capped)).is_err()
    );
    assert!(
        resolve_with_environment(
            Tool::Ffprobe,
            None,
            &fixture.0,
            Path::new("relative program"),
            None
        )
        .is_err()
    );
}

#[test]
fn automatic_and_relative_selections_follow_a_moved_portable_folder() {
    let fixture = Fixture::new();
    let original = fixture.directory("portable original");
    let tools = fixture.directory("portable original/tools");
    let previous = fixture.helper(&tools, Tool::Ffprobe);
    assert_eq!(
        resolve_with_environment(Tool::Ffprobe, None, &original, &original, None).unwrap(),
        previous
    );
    let moved = fixture.0.join("portable moved");
    assert_eq!(original.parent(), Some(fixture.0.as_path()));
    assert_eq!(moved.parent(), Some(fixture.0.as_path()));
    fs::rename(&original, &moved).unwrap();
    let expected = moved
        .join("tools")
        .join(Tool::Ffprobe.filename())
        .canonicalize()
        .unwrap();
    assert_eq!(
        resolve_with_environment(Tool::Ffprobe, None, &moved, &moved, None).unwrap(),
        expected
    );
    let relative = Path::new("tools").join(Tool::Ffprobe.filename());
    assert_eq!(
        resolve_with_environment(Tool::Ffprobe, Some(&relative), &moved, &moved, None).unwrap(),
        expected
    );
    // A previously explicit absolute selection is not silently reinterpreted.
    assert!(
        resolve_with_environment(Tool::Ffprobe, Some(&previous), &moved, &moved, None).is_err()
    );
}

#[test]
fn missing_optional_player_does_not_change_decoder_or_probe_resolution() {
    let fixture = Fixture::new();
    let program = fixture.directory("program");
    let probe = fixture.helper(&program, Tool::Ffprobe);
    let decoder = fixture.helper(&program, Tool::Ffmpeg);
    assert_eq!(
        resolve_with_environment(Tool::Ffprobe, None, &fixture.0, &program, None).unwrap(),
        probe
    );
    assert_eq!(
        resolve_with_environment(Tool::Ffmpeg, None, &fixture.0, &program, None).unwrap(),
        decoder
    );
    assert!(
        resolve_with_environment(Tool::Ffplay, None, &fixture.0, &program, None)
            .unwrap_err()
            .contains("FFplay was not found")
    );
}

#[test]
fn actual_chocolatey_install_precedes_path_shims_and_preserves_bundle_priority() {
    let fixture = Fixture::new();
    let program = fixture.directory("portable app");
    let program_data = fixture.directory("ProgramData");
    let actual = fixture.directory("ProgramData/chocolatey/lib/ffmpeg-full/tools/ffmpeg/bin");
    let shims = fixture.directory("ProgramData/chocolatey/bin");
    let bundled = fixture.directory("portable app/tools");
    let search_path = std::env::join_paths([&shims]).unwrap();
    for tool in [Tool::Ffmpeg, Tool::Ffprobe, Tool::Ffplay] {
        let real = fixture.helper(&actual, tool);
        let shim = fixture.helper(&shims, tool);
        let choose = |explicit| {
            resolve_with_program_data(
                tool,
                explicit,
                &fixture.0,
                &program,
                Some(&search_path),
                Some(&program_data),
            )
        };
        assert_eq!(choose(None).unwrap(), real);
        let portable = fixture.helper(&bundled, tool);
        assert_eq!(choose(None).unwrap(), portable);
        fs::remove_file(portable).unwrap();
        assert_eq!(choose(None).unwrap(), real);
        // An explicit selection is never silently replaced by the known install.
        assert_eq!(choose(Some(shim.as_path())).unwrap(), shim);
        assert!(choose(Some(Path::new("missing-explicit.exe"))).is_err());
    }
}

#[test]
fn discovery_only_uses_fixed_helper_names_and_does_not_execute_or_read_them() {
    let fixture = Fixture::new();
    let program = fixture.directory("program");
    for filename in ["ffmpeg.bat", "ffmpeg.cmd", "ffmpeg.ps1"] {
        fs::write(program.join(filename), b"not a native helper").unwrap();
    }
    assert!(resolve_with_environment(Tool::Ffmpeg, None, &fixture.0, &program, None).is_err());
    let file = fixture.helper(&program, Tool::Ffmpeg);
    // Success with an invalid executable proves discovery is not a launch test.
    assert_eq!(
        resolve_with_environment(Tool::Ffmpeg, None, &fixture.0, &program, None).unwrap(),
        file
    );
    assert!(Tool::from_name("ffmpeg.bat").is_err());
    assert_eq!(Tool::from_name("ffprobe").unwrap(), Tool::Ffprobe);
}
