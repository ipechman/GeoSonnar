//! Media discovery is restricted to the two source folders; selection is never made
//! from a loosely matching filename when multiple inputs are present.
use serde::Serialize;
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Debug, Serialize)]
pub struct MediaInfo {
    pub path: PathBuf,
    pub bytes: u64,
    pub duration_s: f64,
    pub sample_rate_hz: u32,
    pub channels: u32,
    pub audio_codec: String,
    pub metadata: Value,
}

pub fn sidecar_tool(name: &str) -> Result<PathBuf, String> {
    crate::audio::resolve_media_tool(name)
}

/// Media helpers should never create a console window over the native GUI.
pub(crate) fn quiet_command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut command = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    command
}

pub fn discover_raw(project: &Path) -> Result<PathBuf, String> {
    let directory = project.join("Known Videos with coordinates");
    let mut found = Vec::new();
    for entry in fs::read_dir(&directory).map_err(|e| {
        format!(
            "Cannot open {}: {e}. Use --input for another video.",
            directory.display()
        )
    })? {
        let p = entry.map_err(|e| e.to_string())?.path();
        if p.is_file()
            && p.extension().and_then(|s| s.to_str()).is_some_and(|s| {
                matches!(
                    s.to_ascii_lowercase().as_str(),
                    "mp4" | "mkv" | "mov" | "wav" | "flac" | "m4a" | "webm"
                )
            })
        {
            found.push(p);
        }
    }
    match found.len() {
        1 => Ok(found.remove(0)),
        0 => Err(format!(
            "No source recording in {}. Supply --input.",
            directory.display()
        )),
        n => Err(format!(
            "Found {n} source recordings. Supply --input to choose one explicitly."
        )),
    }
}

pub fn inspect(input: &Path) -> Result<MediaInfo, String> {
    inspect_with_probe(input, None)
}

/// An explicit probe supports portable GUI setups without changing process PATH.
pub fn inspect_with_probe(input: &Path, probe: Option<&Path>) -> Result<MediaInfo, String> {
    let path = input
        .canonicalize()
        .map_err(|e| format!("Cannot open {}: {e}", input.display()))?;
    let bytes = fs::metadata(&path).map_err(|e| e.to_string())?.len();
    let explicit_base = std::env::current_dir().map_err(|error| error.to_string())?;
    let selected =
        crate::media_tools::resolve(crate::media_tools::Tool::Ffprobe, probe, &explicit_base)?;
    let mut command = quiet_command(selected);
    let out = command.args(["-v","error","-show_format","-show_streams","-of","json"]).arg(&path).output().map_err(|e|format!("FFprobe is required to inspect video. Choose ffprobe.exe in Settings, or place it beside GeoSonnar: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "FFprobe rejected the input: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let metadata: Value = serde_json::from_slice(&out.stdout)
        .map_err(|e| format!("Invalid FFprobe response: {e}"))?;
    let audio = metadata["streams"]
        .as_array()
        .and_then(|s| s.iter().find(|s| s["codec_type"] == "audio"))
        .ok_or("The selected file has no audio stream")?;
    let duration_s = metadata["format"]["duration"]
        .as_str()
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|s| s.is_finite() && *s > 0.)
        .ok_or("No valid media duration")?;
    let sample_rate_hz = audio["sample_rate"]
        .as_str()
        .and_then(|s| s.parse().ok())
        .ok_or("Missing audio sample rate")?;
    let channels = audio["channels"]
        .as_u64()
        .filter(|n| *n > 0 && *n <= u32::MAX as u64)
        .ok_or("Invalid channel count")? as u32;
    let audio_codec = audio["codec_name"].as_str().unwrap_or("unknown").to_owned();
    Ok(MediaInfo {
        path,
        bytes,
        duration_s,
        sample_rate_hz,
        channels,
        audio_codec,
        metadata,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ambiguous_source_is_not_silently_selected() {
        let base = std::env::temp_dir().join(format!("geosonnar-discover-{}", std::process::id()));
        let raw = base.join("Known Videos with coordinates");
        fs::create_dir_all(&raw).unwrap();
        fs::write(raw.join("a.mp4"), []).unwrap();
        assert!(discover_raw(&base).is_ok());
        fs::write(raw.join("b.mp4"), []).unwrap();
        assert!(discover_raw(&base).unwrap_err().contains("Supply --input"));
        fs::remove_file(raw.join("a.mp4")).unwrap();
        fs::remove_file(raw.join("b.mp4")).unwrap();
        fs::remove_dir(raw).unwrap();
        fs::remove_dir(base).unwrap();
    }
}
