//! Best-effort file logging for pipeline observability.
//!
//! Every artifact is a uniquely-named file under a stage subdirectory of the
//! log root (`logs/file_reads`, `logs/tokenization`, `logs/segmentation`,
//! `logs/generation`). Filenames carry a UTC timestamp prefix so listing a
//! directory is chronological, which doubles as a performance trace. Logging is
//! best-effort: any I/O failure is swallowed — observability never breaks
//! parsing, segmentation, or generation.

use std::fs;
use std::io;
use std::path::PathBuf;

use serde::Serialize;

/// Root of all pipeline logs, resolved once per pipeline run.
#[derive(Clone, Debug)]
pub struct RunLogs {
    root: PathBuf,
}

impl RunLogs {
    /// Resolve the log root: `WORXHHEET_LOG_DIR` if set, else CWD-relative
    /// `logs/` (project root during development).
    pub fn new() -> Option<Self> {
        std::env::var_os("WORXHHEET_LOG_DIR")
            .map(PathBuf::from)
            .or_else(|| Some(PathBuf::from("logs")))
            .map(|root| Self { root })
    }

    /// Stage subdirectory (e.g. `generation`), created on demand.
    fn stage_dir(&self, stage: &str) -> io::Result<PathBuf> {
        let dir = self.root.join(stage);
        fs::create_dir_all(&dir)?;
        Ok(dir)
    }

    /// Path for a new timestamped, uniquely-named file in `stage`.
    fn path_for(&self, stage: &str, label: &str, ext: &str) -> io::Result<PathBuf> {
        let name = format!(
            "{}Z_{}_{}.{}",
            compact_timestamp_utc(),
            ulid::Ulid::new(),
            label,
            ext
        );
        Ok(self.stage_dir(stage)?.join(name))
    }

    /// Write one text artifact. Returns the path on success.
    pub fn write_text(
        &self,
        stage: &str,
        label: &str,
        ext: &str,
        contents: &str,
    ) -> Option<PathBuf> {
        let path = self.path_for(stage, label, ext).ok()?;
        if fs::write(&path, contents).is_err() {
            return None;
        }
        Some(path)
    }

    /// Pretty-print a serializable value into its own JSON artifact.
    pub fn write_json(&self, stage: &str, label: &str, value: &impl Serialize) -> Option<PathBuf> {
        let json = serde_json::to_string_pretty(value).ok()?;
        self.write_text(stage, label, "json", &json)
    }
}

/// Compact, sortable UTC timestamp without colons (filesystem-safe):
/// `20260830T143205.123Z`.
pub fn compact_timestamp_utc() -> String {
    let now = time::OffsetDateTime::now_utc();
    format!(
        "{:04}{:02}{:02}T{:02}{:02}{:02}.{:03}Z",
        now.year(),
        now.month() as u8,
        now.day(),
        now.hour(),
        now.minute(),
        now.second(),
        now.millisecond(),
    )
}

/// Readable UTC timestamp for log record contents.
pub fn rfc3339_utc() -> String {
    use time::format_description::well_known::Rfc3339;
    time::OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| String::from("unknown"))
}

/// Turn an arbitrary label into a filesystem-safe filename fragment.
pub fn sanitize_label(label: &str) -> String {
    let mut out = String::with_capacity(label.len());
    for ch in label.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        out.push_str("unnamed");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Serialize;

    #[derive(Serialize)]
    struct Sample<'a> {
        message: &'a str,
    }

    #[test]
    fn test_write_json_creates_timestamped_file_in_temp_dir() {
        let root = std::env::temp_dir().join(format!("worxheet-logs-{}", ulid::Ulid::new()));
        let logs = RunLogs { root: root.clone() };

        let path = logs
            .write_json("test_stage", "sample", &Sample { message: "hi" })
            .expect("json artifact should write");

        assert_eq!(path.parent().unwrap(), root.join("test_stage"));
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        assert!(
            name.starts_with("20"),
            "filename should carry a UTC timestamp: {name}"
        );
        assert!(
            name.contains("sample.json"),
            "filename should carry the label: {name}"
        );

        let contents = fs::read_to_string(&path).unwrap();
        assert!(contents.contains("\"message\": \"hi\""));
    }

    #[test]
    fn test_timestamps_are_column_free_and_sorted() {
        let a = compact_timestamp_utc();
        let b = compact_timestamp_utc();
        assert!(!a.contains(':'));
        assert_eq!(a.len(), b.len());
        assert!(a <= b, "timestamps should be lexically sortable");
        assert!(!rfc3339_utc().is_empty());
    }
}
