//! Synthetic review-media controls; installed-helper checks are opt-in and do
//! not use private recordings, event labels, or evaluation locations.
use geosonnar::review_media::{self, FrameRequest, WindowRequest};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

static NEXT: AtomicU64 = AtomicU64::new(0);

fn directory() -> PathBuf {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("output/benchmarks/review-media")
        .join(format!(
            "{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
    fs::create_dir_all(&path).unwrap();
    path
}

fn request() -> WindowRequest {
    WindowRequest {
        input: PathBuf::from("missing.wav"),
        ffmpeg: PathBuf::from("missing-ffmpeg"),
        ffprobe: PathBuf::from("missing-ffprobe"),
        channel: 0,
        start_s: 0.,
        end_s: 10.,
        fft_size: 4096,
        hop_s: 0.1,
        max_frequency_hz: 800.,
    }
}

fn helpers() -> (PathBuf, PathBuf) {
    (
        std::env::var_os("GEOSONNAR_TEST_FFMPEG")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                "C:/ProgramData/chocolatey/lib/ffmpeg-full/tools/ffmpeg/bin/ffmpeg.exe".into()
            }),
        std::env::var_os("GEOSONNAR_TEST_FFPROBE")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                "C:/ProgramData/chocolatey/lib/ffmpeg-full/tools/ffmpeg/bin/ffprobe.exe".into()
            }),
    )
}

fn stereo_wave(path: &Path) {
    let rate = 8000u32;
    let frames = rate * 4;
    let size = frames * 4;
    let mut bytes = Vec::new();
    bytes.extend(b"RIFF");
    bytes.extend((36 + size).to_le_bytes());
    bytes.extend(b"WAVEfmt ");
    bytes.extend(16u32.to_le_bytes());
    bytes.extend(1u16.to_le_bytes());
    bytes.extend(2u16.to_le_bytes());
    bytes.extend(rate.to_le_bytes());
    bytes.extend((rate * 4).to_le_bytes());
    bytes.extend(4u16.to_le_bytes());
    bytes.extend(16u16.to_le_bytes());
    bytes.extend(b"data");
    bytes.extend(size.to_le_bytes());
    for i in 0..frames {
        let t = f64::from(i) / f64::from(rate);
        for phase in [500. * t, 220. * t - 10. * t * t] {
            let sample = (16000. * (std::f64::consts::TAU * phase).sin()).round() as i16;
            bytes.extend(sample.to_le_bytes());
        }
    }
    fs::write(path, bytes).unwrap();
}

#[test]
fn invalid_intervals_channels_fft_hops_and_budgets_fail_before_launch() {
    request().validate().unwrap();
    for variant in 0..11 {
        let mut r = request();
        match variant {
            0 => r.start_s = -1.,
            1 => r.end_s = f64::NAN,
            2 => r.end_s = 301.,
            3 => r.fft_size = 1000,
            4 => r.fft_size = 32768,
            5 => r.hop_s = 0.,
            6 => r.max_frequency_hz = 2001.,
            7 => r.channel = 64,
            8 => r.end_s = 0.5,
            9 => {
                r.end_s = 300.;
                r.hop_s = 0.00025;
            }
            _ => {
                r.end_s = 300.;
                r.hop_s = 0.00025;
                r.max_frequency_hz = 0.01;
            }
        }
        let error = review_media::analyze_window(&r).unwrap_err();
        assert!(
            !error.contains("Recording unavailable"),
            "Invalid request reached filesystem: {variant}"
        );
    }
    let bad_frame = FrameRequest {
        input: "missing".into(),
        ffmpeg: "missing".into(),
        ffprobe: "missing".into(),
        time_s: 0.,
        max_width: 1921,
        max_height: 1080,
        expected_source: None,
    };
    assert!(
        review_media::extract_frame(&bad_frame)
            .unwrap_err()
            .contains("preview size")
    );
}

#[test]
fn source_stamp_binds_nonempty_file_and_detects_changed_size() {
    let root = directory();
    let source = root.join("source é.raw");
    fs::write(&source, b"source").unwrap();
    let before = review_media::source_stamp(&source).unwrap();
    assert_eq!(before.bytes, 6);
    assert!(before.path.is_absolute());
    fs::write(&source, b"source changed").unwrap();
    assert_ne!(before, review_media::source_stamp(&source).unwrap());
    assert!(review_media::source_stamp(&root).is_err());
}

#[test]
#[ignore = "Requires installed FFmpeg/FFprobe; creates synthetic chirp audio only"]
fn actual_stereo_chirp_windows_keep_channel_time_frequency_and_rms() {
    let root = directory();
    let source = root.join("stereo chirp é.wav");
    stereo_wave(&source);
    let (ffmpeg, ffprobe) = helpers();
    let mut r = WindowRequest {
        input: source.clone(),
        ffmpeg: ffmpeg.clone(),
        ffprobe: ffprobe.clone(),
        channel: 1,
        start_s: 1.0001,
        end_s: 3.1,
        fft_size: 1024,
        hop_s: 0.128,
        max_frequency_hz: 800.,
    };
    let chirp = review_media::analyze_window(&r).unwrap();
    assert_eq!(chirp.start_s, 1.00025);
    assert_eq!(chirp.end_s, 3.1);
    assert_eq!(chirp.times_s[0], 1.12825);
    assert_eq!(chirp.decoded_samples, 8399);
    let bins = chirp.frequencies_hz.len();
    for (i, row) in chirp.power_db.chunks(bins).enumerate() {
        let peak = row
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .unwrap()
            .0;
        assert!((chirp.frequencies_hz[peak] - (220. - 20. * chirp.times_s[i])).abs() < 8.);
        assert!((chirp.envelope[i].rms - 16000. / 32768. / 2_f64.sqrt()).abs() < 0.01);
    }
    r.channel = 0;
    let tone = review_media::analyze_window(&r).unwrap();
    for row in tone.power_db.chunks(bins) {
        let peak = row
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .unwrap()
            .0;
        assert_eq!(tone.frequencies_hz[peak], 500.);
    }
    assert!(tone.envelope.iter().all(|point| point.rms < 1e-4));
    assert_eq!(tone.source_sha256, chirp.source_sha256);
    assert!(
        review_media::extract_frame(&FrameRequest {
            input: source,
            ffmpeg,
            ffprobe,
            time_s: 1.,
            max_width: 320,
            max_height: 180,
            expected_source: Some(chirp.source_stamp)
        })
        .unwrap()
        .is_none()
    );
    r.channel = 2;
    assert!(
        review_media::analyze_window(&r)
            .unwrap_err()
            .contains("channel")
    );
}

#[test]
#[ignore = "Requires installed FFmpeg/FFprobe; creates synthetic video/audio only"]
fn actual_video_frame_reports_decoded_timestamp_aspect_and_source_stamp() {
    let root = directory();
    let source = root.join("synthetic video.mkv");
    let (ffmpeg, ffprobe) = helpers();
    let mut command = std::process::Command::new(&ffmpeg);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let output = command
        .args([
            "-v",
            "error",
            "-nostdin",
            "-n",
            "-f",
            "lavfi",
            "-i",
            "testsrc=size=160x90:rate=10:duration=4",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=125:sample_rate=8000:duration=4",
            "-c:v",
            "ffv1",
            "-pix_fmt",
            "yuv444p10le",
            "-c:a",
            "pcm_s16le",
            "-shortest",
        ])
        .arg(&source)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut r = FrameRequest {
        input: source,
        ffmpeg,
        ffprobe,
        time_s: 1.05,
        max_width: 80,
        max_height: 80,
        expected_source: None,
    };
    let frame = review_media::extract_frame(&r).unwrap().unwrap();
    assert_eq!((frame.width, frame.height), (80, 45));
    assert_eq!(frame.rgb.len(), 80 * 45 * 3);
    assert!(
        (frame.frame_time_s - 1.1).abs() < 0.001,
        "Actual frame timestamp {}",
        frame.frame_time_s
    );
    assert_eq!(frame.requested_time_s, 1.05);
    r.expected_source = Some(frame.source_stamp.clone());
    r.time_s = 2.31;
    let later = review_media::extract_frame(&r).unwrap().unwrap();
    assert!((later.frame_time_s - 2.4).abs() < 0.001);
    r.expected_source.as_mut().unwrap().bytes += 1;
    assert!(
        review_media::extract_frame(&r)
            .unwrap_err()
            .contains("changed")
    );
}

#[test]
#[ignore = "Requires installed FFmpeg/FFprobe; verifies delayed first audio against another default stream"]
fn shifted_first_audio_stream_stays_aligned_with_frame_time_and_spectrum() {
    let root = directory();
    let source = root.join("delayed audio with alternate default.mkv");
    let (ffmpeg, ffprobe) = helpers();
    let mut command = std::process::Command::new(&ffmpeg);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let output = command
        .args([
            "-v",
            "error",
            "-nostdin",
            "-n",
            "-f",
            "lavfi",
            "-i",
            "testsrc=size=160x90:rate=10:duration=5",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=125:sample_rate=8000:duration=4",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=500:sample_rate=8000:duration=5",
            "-filter_complex",
            "[1:a]asetpts=PTS+1/TB[delayed]",
            "-map",
            "0:v",
            "-map",
            "[delayed]",
            "-map",
            "2:a",
            "-disposition:a:0",
            "0",
            "-disposition:a:1",
            "default",
            "-c:v",
            "ffv1",
            "-c:a",
            "pcm_s16le",
        ])
        .arg(&source)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let window = review_media::analyze_window(&WindowRequest {
        input: source.clone(),
        ffmpeg: ffmpeg.clone(),
        ffprobe: ffprobe.clone(),
        channel: 0,
        start_s: 0.,
        end_s: 2.,
        fft_size: 1024,
        hop_s: 0.128,
        max_frequency_hz: 800.,
    })
    .unwrap();
    assert_eq!(window.times_s[0], 0.128);
    let bins = window.frequencies_hz.len();
    for row in window.power_db.chunks(bins) {
        let peak = row
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .unwrap()
            .0;
        assert_eq!(window.frequencies_hz[peak], 125.);
    }
    let frame = review_media::extract_frame(&FrameRequest {
        input: source,
        ffmpeg,
        ffprobe,
        time_s: 0.05,
        max_width: 160,
        max_height: 90,
        expected_source: Some(window.source_stamp),
    })
    .unwrap()
    .unwrap();
    assert!(
        (frame.frame_time_s - 0.1).abs() < 0.001,
        "Audio-relative frame timestamp {}",
        frame.frame_time_s
    );
}
