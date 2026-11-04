//! Bounded native HTTPS transport shared by explicit provider requests.
use std::{
    fs::{self, File, Metadata, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const COPY_BUFFER_BYTES: usize = 16 * 1024;
const STATUS_LIMIT_BYTES: u64 = 16;
const TRANSFER_TIMEOUT: Duration = Duration::from_secs(45);
const MONITOR_INTERVAL: Duration = Duration::from_millis(20);

/// The supplied path is a precreated empty regular file owned by this attempt.
/// At most limit + 1 bytes are retained. The last byte proves an overrun and
/// must be charged to the attempt's received-byte budget; it is never success.
#[derive(Clone, Debug)]
pub struct TransportResult {
    pub http_status: Option<u16>,
    pub error: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
enum CopyOutcome {
    Complete(u64),
    LimitExceeded(u64),
}

/// Bound even endless streams and old curl versions' chunked responses.
/// Already-written bytes are deliberately retained on every failure.
fn copy_bounded<R: Read, W: Write>(
    reader: &mut R,
    writer: &mut W,
    limit: u64,
) -> io::Result<CopyOutcome> {
    let ceiling = limit.checked_add(1).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "Response byte limit is too large",
        )
    })?;
    let mut copied = 0;
    let mut buffer = [0_u8; COPY_BUFFER_BYTES];
    loop {
        let available = (ceiling - copied).min(buffer.len() as u64) as usize;
        let count = match reader.read(&mut buffer[..available]) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            result => result?,
        };
        if count == 0 {
            writer.flush()?;
            return Ok(CopyOutcome::Complete(copied));
        }
        writer.write_all(&buffer[..count])?;
        copied += count as u64;
        if copied == ceiling {
            writer.flush()?;
            return Ok(CopyOutcome::LimitExceeded(copied));
        }
    }
}

fn regular_file(metadata: &Metadata) -> bool {
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return false;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return false;
        }
    }
    true
}

/// Never create, truncate, follow a final Windows reparse point, or overwrite a
/// nonempty body. Check the open handle as well as the supplied path.
fn open_empty_body(path: &Path) -> Result<File, String> {
    let metadata = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !regular_file(&metadata) || metadata.len() != 0 {
        return Err(
            "Download body must be a precreated empty regular file without reparse points".into(),
        );
    }
    let mut options = OpenOptions::new();
    options.write(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // FILE_FLAG_OPEN_REPARSE_POINT. Exclusive sharing prevents replacement
        // and concurrent writes while this attempt owns the body handle.
        options.custom_flags(0x0020_0000).share_mode(0);
    }
    let file = options.open(path).map_err(|e| e.to_string())?;
    let opened = file.metadata().map_err(|e| e.to_string())?;
    let current = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !regular_file(&opened) || opened.len() != 0 || !regular_file(&current) {
        return Err("Download body changed or is not an empty regular file".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if opened.dev() != metadata.dev()
            || opened.ino() != metadata.ino()
            || opened.dev() != current.dev()
            || opened.ino() != current.ino()
        {
            return Err("Download body changed while opening it".into());
        }
    }
    Ok(file)
}

fn read_status(path: &Path) -> Result<Option<u16>, String> {
    let metadata = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !regular_file(&metadata) || metadata.len() > STATUS_LIMIT_BYTES {
        return Err("Downloader status is not a bounded regular file".into());
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x0020_0000);
    }
    let file = options.open(path).map_err(|e| e.to_string())?;
    if !regular_file(&file.metadata().map_err(|e| e.to_string())?) {
        return Err("Downloader status changed to a non-regular file".into());
    }
    let mut bytes = Vec::new();
    file.take(STATUS_LIMIT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > STATUS_LIMIT_BYTES {
        return Err("Downloader status exceeded its byte limit".into());
    }
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| "Downloader status is not UTF-8")?
        .trim();
    if text.is_empty() || text == "000" {
        return Ok(None);
    }
    if text.len() != 3 || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("Downloader returned an invalid HTTP status".into());
    }
    let code = text.parse::<u16>().map_err(|e| e.to_string())?;
    if !(100..=599).contains(&code) {
        return Err("Downloader returned an invalid HTTP status".into());
    }
    Ok(Some(code))
}

fn append_error(error: &mut Option<String>, extra: String) {
    match error {
        Some(existing) if !existing.contains(&extra) => {
            existing.push_str("; ");
            existing.push_str(&extra);
        }
        Some(_) => {}
        None => *error = Some(extra),
    }
}

fn copy_error(result: &Result<CopyOutcome, String>) -> Option<String> {
    match result {
        Ok(CopyOutcome::Complete(_)) => None,
        Ok(CopyOutcome::LimitExceeded(bytes)) => Some(format!(
            "Download exceeded its byte limit; {bytes} received bytes retained, including one overrun byte"
        )),
        Err(error) => Some(error.clone()),
    }
}

fn join_reader(reader: JoinHandle<Result<CopyOutcome, String>>) -> Result<CopyOutcome, String> {
    reader.join().map_err(|_| {
        "Download body reader stopped unexpectedly; partial bytes retained".to_string()
    })?
}

fn stop_owned_child(child: &mut Child, error: &mut Option<String>) {
    // Kill only this Command's process. Reap before joining its pipe reader.
    if let Err(kill_error) = child.kill()
        && !matches!(child.try_wait(), Ok(Some(_)))
    {
        append_error(error, format!("Cannot stop downloader: {kill_error}"));
    }
    if let Err(wait_error) = child.wait() {
        append_error(error, format!("Cannot reap downloader: {wait_error}"));
    }
}

/// No shell, cookies, curl configuration or credential stores. The application
/// enforces the byte cap independently of curl, and monitors a 45-second limit.
pub(crate) fn download(
    url: &str,
    limit: u64,
    body: &Path,
    cancel: &AtomicBool,
) -> Result<TransportResult, String> {
    limit
        .checked_add(1)
        .ok_or("Response byte limit is too large")?;
    let mut body_file = open_empty_body(body)?;
    if cancel.load(Ordering::Relaxed) {
        return Ok(TransportResult {
            http_status: None,
            error: Some("Download cancelled before transfer; attempt body retained".into()),
        });
    }
    let executable = if cfg!(windows) {
        PathBuf::from(std::env::var_os("SystemRoot").ok_or("Windows system directory unavailable")?)
            .join("System32/curl.exe")
    } else {
        PathBuf::from("curl")
    };
    let status_path = body.with_extension("status");
    let status_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&status_path)
        .map_err(|e| format!("Cannot create exclusive downloader status: {e}"))?;
    let mut command = Command::new(executable);
    command
        .args([
            "-q",
            "--silent",
            "--fail",
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--max-redirs",
            "0",
            "--connect-timeout",
            "15",
            "--max-time",
            "45",
            "--max-filesize",
            &limit.to_string(),
            "--write-out",
            // Documented by https://curl.se/docs/manpage.html#-w .
            "%{stderr}%{http_code}",
            "--output",
            "-",
        ])
        .arg("--")
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(status_file);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let spawned = command.spawn();
    // Close Command's copies of its stdio handles before joining the reader.
    drop(command);
    let mut child = spawned.map_err(|e| format!("Cannot start native HTTPS downloader: {e}"))?;
    let mut stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => {
            let mut error = Some("Downloader did not provide its body pipe".into());
            stop_owned_child(&mut child, &mut error);
            return Err(error.unwrap());
        }
    };
    let reader = thread::Builder::new()
        .name("bounded-http-body".into())
        .spawn(move || {
            let outcome = copy_bounded(&mut stdout, &mut body_file, limit)
                .map_err(|e| format!("Cannot retain download body: {e}; partial bytes retained"))?;
            body_file
                .sync_all()
                .map_err(|e| format!("Cannot sync download body: {e}; partial bytes retained"))?;
            Ok(outcome)
        });
    let mut reader = match reader {
        Ok(reader) => Some(reader),
        Err(spawn_error) => {
            let mut error = Some(format!("Cannot start download body reader: {spawn_error}"));
            stop_owned_child(&mut child, &mut error);
            return Err(error.unwrap());
        }
    };
    let started = Instant::now();
    let mut copied = None;
    let mut child_exited = false;
    let mut error = loop {
        if cancel.load(Ordering::Relaxed) {
            break Some("Download cancelled; received bytes retained".into());
        }
        if started.elapsed() >= TRANSFER_TIMEOUT {
            break Some(
                "Download exceeded its 45-second time limit; received bytes retained".into(),
            );
        }
        if reader.as_ref().is_some_and(|reader| reader.is_finished()) {
            copied = Some(join_reader(reader.take().unwrap()));
            if let Some(error) = copy_error(copied.as_ref().unwrap()) {
                break Some(error);
            }
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                child_exited = true;
                break if status.success() {
                    None
                } else {
                    Some(format!(
                        "HTTPS download failed (exit {:?}); file may be absent, unavailable, or over its byte limit",
                        status.code()
                    ))
                };
            }
            Ok(None) => thread::sleep(MONITOR_INTERVAL),
            Err(monitor_error) => {
                break Some(format!("Cannot monitor downloader: {monitor_error}"));
            }
        }
    };
    if !child_exited {
        stop_owned_child(&mut child, &mut error);
    }
    // curl owns the sole writer. Exit/kill plus reap above closes it before a
    // join; the remaining pipe bytes can only be drained up to limit + 1.
    if let Some(reader) = reader {
        copied = Some(join_reader(reader));
    }
    if let Some(extra) = copied.as_ref().and_then(copy_error) {
        append_error(&mut error, extra);
    }
    let http_status = match read_status(&status_path) {
        Ok(status) => status,
        Err(status_error) => {
            append_error(&mut error, status_error);
            None
        }
    };
    Ok(TransportResult { http_status, error })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Cursor, sync::atomic::AtomicU64};

    struct TempDirectory(PathBuf);
    impl TempDirectory {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "geosonnar-http-{}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for TempDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn exact_and_short_bodies_keep_every_byte_without_a_sentinel() {
        for length in [
            0,
            1,
            COPY_BUFFER_BYTES - 1,
            COPY_BUFFER_BYTES,
            COPY_BUFFER_BYTES + 5,
        ] {
            let input = vec![0x7d; length];
            for limit in [length as u64, length as u64 + 17] {
                let mut output = Vec::new();
                assert_eq!(
                    copy_bounded(&mut Cursor::new(&input), &mut output, limit).unwrap(),
                    CopyOutcome::Complete(length as u64)
                );
                assert_eq!(output, input);
            }
        }
    }

    #[test]
    fn endless_body_keeps_exactly_one_charged_overrun_byte() {
        for limit in [
            0,
            1,
            COPY_BUFFER_BYTES as u64 - 1,
            COPY_BUFFER_BYTES as u64 + 3,
        ] {
            let mut output = Vec::new();
            assert_eq!(
                copy_bounded(&mut io::repeat(0xa7), &mut output, limit).unwrap(),
                CopyOutcome::LimitExceeded(limit + 1)
            );
            assert_eq!(output, vec![0xa7; limit as usize + 1]);
        }
    }

    #[test]
    fn impossible_limit_is_rejected_before_read_or_write() {
        let mut input = Cursor::new(b"untouched".to_vec());
        let mut output = Vec::new();
        let error = copy_bounded(&mut input, &mut output, u64::MAX).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(input.position(), 0);
        assert!(output.is_empty());
    }

    struct ReadFailure(Cursor<Vec<u8>>);
    impl Read for ReadFailure {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            if self.0.position() == self.0.get_ref().len() as u64 {
                return Err(io::Error::other("injected read failure"));
            }
            self.0.read(buffer)
        }
    }

    #[test]
    fn read_failure_retains_the_received_prefix() {
        let mut input = ReadFailure(Cursor::new(b"received-prefix".to_vec()));
        let mut output = Vec::new();
        let error = copy_bounded(&mut input, &mut output, 100).unwrap_err();
        assert!(error.to_string().contains("injected read failure"));
        assert_eq!(output, b"received-prefix");
    }

    struct WriteFailure {
        prefix: Vec<u8>,
        remaining: usize,
    }
    impl Write for WriteFailure {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            if self.remaining == 0 {
                return Err(io::Error::other("injected write failure"));
            }
            let count = self.remaining.min(buffer.len());
            self.prefix.extend_from_slice(&buffer[..count]);
            self.remaining -= count;
            Ok(count)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn partial_write_failure_preserves_written_bytes() {
        let mut output = WriteFailure {
            prefix: Vec::new(),
            remaining: 4,
        };
        let error = copy_bounded(&mut Cursor::new(b"abcdefghij"), &mut output, 100).unwrap_err();
        assert!(error.to_string().contains("injected write failure"));
        assert_eq!(output.prefix, b"abcd");
    }

    struct FlushFailure(Vec<u8>);
    impl Write for FlushFailure {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            self.0.extend_from_slice(buffer);
            Ok(buffer.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Err(io::Error::other("injected flush failure"))
        }
    }

    #[test]
    fn flush_failure_is_not_reported_as_a_successful_body() {
        let mut output = FlushFailure(Vec::new());
        let error = copy_bounded(&mut Cursor::new(b"complete"), &mut output, 8).unwrap_err();
        assert!(error.to_string().contains("injected flush failure"));
        assert_eq!(output.0, b"complete");
    }

    #[test]
    fn body_open_never_creates_or_truncates_an_existing_artifact() {
        let directory = TempDirectory::new();
        let path = directory.0.join("response.bin");
        assert!(open_empty_body(&path).is_err());
        assert!(!path.exists());
        fs::write(&path, b"preserved original").unwrap();
        assert!(open_empty_body(&path).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"preserved original");
        assert!(open_empty_body(&directory.0).is_err());
        let empty = directory.0.join("empty.bin");
        fs::write(&empty, []).unwrap();
        let mut file = open_empty_body(&empty).unwrap();
        let result = copy_bounded(&mut io::repeat(7), &mut file, 4).unwrap();
        drop(file);
        assert_eq!(result, CopyOutcome::LimitExceeded(5));
        assert_eq!(fs::read(&empty).unwrap(), vec![7; 5]);
    }

    #[test]
    fn status_read_is_bounded_and_rejects_malformed_codes() {
        let directory = TempDirectory::new();
        let path = directory.0.join("response.status");
        for (text, expected) in [
            ("", None),
            ("000", None),
            ("200", Some(200)),
            (" 404\n", Some(404)),
        ] {
            fs::write(&path, text).unwrap();
            assert_eq!(read_status(&path).unwrap(), expected);
        }
        for text in [
            "20",
            "2xx",
            "999",
            "200200",
            "200 followed by an oversized annotation",
        ] {
            fs::write(&path, text).unwrap();
            assert!(read_status(&path).is_err());
            assert_eq!(fs::read_to_string(&path).unwrap(), text);
        }
    }

    #[test]
    fn already_cancelled_transfer_keeps_empty_body_without_starting_a_process() {
        let directory = TempDirectory::new();
        let body = directory.0.join("response.bin");
        fs::write(&body, []).unwrap();
        let outcome =
            download("https://unused.invalid/", 20, &body, &AtomicBool::new(true)).unwrap();
        assert_eq!(outcome.http_status, None);
        assert!(outcome.error.unwrap().contains("cancelled before transfer"));
        assert_eq!(fs::metadata(&body).unwrap().len(), 0);
        assert!(!body.with_extension("status").exists());
    }

    #[test]
    fn existing_status_is_preserved_and_prevents_starting_a_process() {
        let directory = TempDirectory::new();
        let body = directory.0.join("response.bin");
        let status = body.with_extension("status");
        fs::write(&body, []).unwrap();
        fs::write(&status, b"previous receipt").unwrap();
        let error = download(
            "https://unused.invalid/",
            20,
            &body,
            &AtomicBool::new(false),
        )
        .unwrap_err();
        assert!(error.contains("exclusive downloader status"));
        assert_eq!(fs::read(&status).unwrap(), b"previous receipt");
        assert_eq!(fs::metadata(&body).unwrap().len(), 0);
    }
}
