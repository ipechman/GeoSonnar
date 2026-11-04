//! Shared media-helper discovery. This module checks filesystem metadata only.
//! Finding a file does not execute it, validate its version, or certify its safety.
//! Automatic selections are resolved afresh; callers should persist `None`, not
//! the discovered absolute path, so moving a portable folder remains supported.

use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
};

/// Prevent a malformed or enormous PATH from turning a refresh into an
/// unbounded filesystem walk. Entries beyond this prefix are not inspected.
pub const MAX_PATH_ENTRIES: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Ffmpeg,
    Ffprobe,
    Ffplay,
}

impl Tool {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Ffmpeg => "FFmpeg",
            Self::Ffprobe => "FFprobe",
            Self::Ffplay => "FFplay",
        }
    }

    pub const fn filename(self) -> &'static str {
        match (self, cfg!(windows)) {
            (Self::Ffmpeg, true) => "ffmpeg.exe",
            (Self::Ffprobe, true) => "ffprobe.exe",
            (Self::Ffplay, true) => "ffplay.exe",
            (Self::Ffmpeg, false) => "ffmpeg",
            (Self::Ffprobe, false) => "ffprobe",
            (Self::Ffplay, false) => "ffplay",
        }
    }

    pub fn from_name(name: &str) -> Result<Self, String> {
        match name {
            "ffmpeg" => Ok(Self::Ffmpeg),
            "ffprobe" => Ok(Self::Ffprobe),
            "ffplay" => Ok(Self::Ffplay),
            _ => Err(format!("Unsupported media helper: {name}")),
        }
    }
}

/// Resolve an explicit path or discover a helper beside the running program.
/// `base` resolves explicit relative paths only. `None` means automatic;
/// `Some(empty)` is an invalid explicit selection, never permission to fall back.
///
/// Metadata access can wait on slow filesystems. GUI callers should run this
/// outside the UI thread and cache its result until an explicit refresh/action.
pub fn resolve(tool: Tool, configured: Option<&Path>, base: &Path) -> Result<PathBuf, String> {
    // An explicit selection must not depend on successful automatic discovery.
    if configured.is_some() {
        return resolve_with_environment(tool, configured, base, Path::new(""), None);
    }
    let executable = std::env::current_exe().map_err(|error| {
        format!(
            "Cannot locate GeoSonnar for {} discovery: {error}",
            tool.label()
        )
    })?;
    let directory = executable
        .parent()
        .ok_or("GeoSonnar has no executable directory")?;
    let search_path = std::env::var_os("PATH");
    let program_data = std::env::var_os("ProgramData");
    resolve_with_program_data(
        tool,
        None,
        base,
        directory,
        search_path.as_deref(),
        program_data.as_deref().map(Path::new),
    )
}

/// Deterministic resolver for callers supplying their executable location and
/// for offline controls. Order: `tools/`, executable directory, absolute PATH
/// entries in their declared order (at most256). Empty/relative and Windows
/// network-drive/UNC automatic locations are skipped.
/// No program is launched and no helper file contents are read.
pub fn resolve_with_environment(
    tool: Tool,
    configured: Option<&Path>,
    base: &Path,
    executable_directory: &Path,
    search_path: Option<&OsStr>,
) -> Result<PathBuf, String> {
    resolve_with_program_data(
        tool,
        configured,
        base,
        executable_directory,
        search_path,
        None,
    )
}

/// Same deterministic metadata-only resolution with an injected ProgramData
/// location. Preserve the GUI's known actual Chocolatey ffmpeg-full installation
/// before searching PATH, where Chocolatey's executable shims commonly reside.
/// Explicit selections still bypass every automatic fallback.
pub fn resolve_with_program_data(
    tool: Tool,
    configured: Option<&Path>,
    base: &Path,
    executable_directory: &Path,
    search_path: Option<&OsStr>,
    program_data: Option<&Path>,
) -> Result<PathBuf, String> {
    if let Some(path) = configured {
        if path.as_os_str().is_empty() {
            return Err(format!(
                "The explicit {} path is empty. Choose a file or select Auto-detect.",
                tool.label()
            ));
        }
        let selected = if path.is_absolute() {
            path.to_path_buf()
        } else {
            if !base.is_absolute() {
                return Err(
                    "Explicit relative media-tool paths require an absolute base directory".into(),
                );
            }
            base.join(path)
        };
        return regular_file(&selected).map_err(|error| {
            format!(
                "Select the actual {} executable: {error} at {}. Choose another file or select Auto-detect; the explicit selection was not replaced.",
                tool.label(), selected.display()
            )
        });
    }
    if !executable_directory.is_absolute() {
        return Err(
            "Automatic media-tool discovery requires an absolute executable directory".into(),
        );
    }
    let filename = tool.filename();
    let bundled = [
        executable_directory.join("tools").join(filename),
        executable_directory.join(filename),
    ];
    let installed = program_data.map(|directory| {
        directory
            .join("chocolatey/lib/ffmpeg-full/tools/ffmpeg/bin")
            .join(filename)
    });
    for candidate in bundled.into_iter().chain(installed).chain(
        search_path
            .into_iter()
            .flat_map(std::env::split_paths)
            .take(MAX_PATH_ENTRIES)
            .filter(|directory| automatic_location_is_local(directory))
            .map(|directory| directory.join(filename)),
    ) {
        // A directory named like a helper does not mask later valid files.
        if !automatic_candidate_is_file(&candidate) {
            continue;
        }
        // Once selected, report a race/access error instead of silently changing
        // helpers between the metadata check and canonical path resolution.
        return regular_file(&candidate).map_err(|error| {
            format!(
                "Cannot resolve detected {} at {}: {error}",
                tool.label(),
                candidate.display()
            )
        });
    }
    Err(format!(
        "{} was not found. Place {} in GeoSonnar's tools folder or beside the program, install it on a local absolute PATH entry, or choose its actual executable in Settings. Automatic discovery also checks the actual Chocolatey ffmpeg-full installation. It checks at most256 PATH entries and skips direct Windows network locations and detected symbolic/reparse links. Media helpers are not included in this package.",
        tool.label(),
        filename
    ))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AutomaticEntry {
    Directory,
    File,
    Reparse,
    Other,
}

fn automatic_candidate_is_file(path: &Path) -> bool {
    automatic_candidate_with(path, |part| {
        let metadata = std::fs::symlink_metadata(part).ok()?;
        #[cfg(windows)]
        let reparse = {
            use std::os::windows::fs::MetadataExt;
            metadata.file_attributes() & 0x400 != 0
        };
        #[cfg(not(windows))]
        let reparse = false;
        Some(if reparse || metadata.file_type().is_symlink() {
            AutomaticEntry::Reparse
        } else if metadata.is_dir() {
            AutomaticEntry::Directory
        } else if metadata.is_file() {
            AutomaticEntry::File
        } else {
            AutomaticEntry::Other
        })
    })
}

fn automatic_candidate_with(
    path: &Path,
    mut inspect: impl FnMut(&Path) -> Option<AutomaticEntry>,
) -> bool {
    if !automatic_location_is_local(path) {
        return false;
    }
    // Inspect from the root downward without following each inspected link,
    // stopping before looking up a descendant of a reparse directory. This is
    // a fresh metadata check, not an atomic guard against concurrent renames.
    let ancestors: Vec<_> = path.ancestors().collect();
    for part in ancestors.into_iter().rev() {
        let expected = if part == path {
            AutomaticEntry::File
        } else {
            AutomaticEntry::Directory
        };
        if inspect(part) != Some(expected) {
            return false;
        }
    }
    true
}

#[cfg(windows)]
fn automatic_location_is_local(path: &Path) -> bool {
    use std::path::{Component, Prefix};
    if !path.is_absolute() {
        return false;
    }
    let Some(Component::Prefix(prefix)) = path.components().next() else {
        return false;
    };
    let letter = match prefix.kind() {
        Prefix::Disk(letter) | Prefix::VerbatimDisk(letter) => letter,
        // UNC and device paths are not contacted during automatic discovery.
        _ => return false,
    };
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetDriveTypeW(root: *const u16) -> u32;
    }
    let root = [u16::from(letter), b':' as u16, b'\\' as u16, 0];
    // DRIVE_REMOVABLE/FIXED/CDROM/RAMDISK only. This checks the drive mapping,
    // not a file on a remote share; unavailable and remote drives are skipped.
    matches!(unsafe { GetDriveTypeW(root.as_ptr()) }, 2 | 3 | 5 | 6)
}

#[cfg(not(windows))]
fn automatic_location_is_local(path: &Path) -> bool {
    path.is_absolute()
}

fn regular_file(path: &Path) -> Result<PathBuf, String> {
    let resolved = path.canonicalize().map_err(|error| error.to_string())?;
    if !resolved.is_file() {
        return Err("the selected path is not a file".into());
    }
    Ok(resolved)
}

#[cfg(test)]
mod automatic_ancestor_controls {
    use super::*;

    #[test]
    fn detected_reparse_ancestor_stops_before_any_descendant_lookup() {
        let base = std::env::current_dir().unwrap();
        let link = base.join("synthetic-network-link");
        let candidate = link.join("deeper").join(Tool::Ffprobe.filename());
        let mut visited = Vec::new();
        assert!(!automatic_candidate_with(&candidate, |part| {
            visited.push(part.to_path_buf());
            Some(if part == link.as_path() {
                AutomaticEntry::Reparse
            } else {
                AutomaticEntry::Directory
            })
        }));
        assert_eq!(visited.last(), Some(&link));
        assert!(
            !visited
                .iter()
                .any(|path| path != &link && path.starts_with(&link))
        );
    }

    #[test]
    fn automatic_file_links_are_refused_while_ordinary_files_are_accepted() {
        let candidate = std::env::current_dir()
            .unwrap()
            .join("synthetic-helper.exe");
        for (kind, expected) in [
            (AutomaticEntry::Reparse, false),
            (AutomaticEntry::File, true),
        ] {
            assert_eq!(
                automatic_candidate_with(&candidate, |part| {
                    Some(if part == candidate.as_path() {
                        kind
                    } else {
                        AutomaticEntry::Directory
                    })
                }),
                expected
            );
        }
    }
}

#[cfg(all(test, windows))]
mod windows_path_controls {
    use super::*;

    #[test]
    fn automatic_discovery_never_contacts_unc_device_or_relative_locations() {
        for path in [
            r"\\unreachable.invalid\share\tools",
            r"\\?\UNC\unreachable.invalid\share\tools",
            r"\\.\pipe\not-a-media-tool",
            r"relative\tools",
            "",
        ] {
            assert!(!automatic_location_is_local(Path::new(path)));
        }
    }
}
