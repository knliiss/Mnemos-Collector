use std::io::SeekFrom;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use same_file::Handle;
use tokio::fs::File;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncSeekExt, BufReader};

use crate::diagnostics;

#[derive(Debug)]
pub struct LogTailer {
    path: PathBuf,
    identity: Handle,
    file: File,
    offset: u64,
    pending: Vec<u8>,
    generation: u64,
}

impl LogTailer {
    pub async fn open_from_end(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let identity = Handle::from_path(&path)
            .with_context(|| format!("failed to identify {}", path.display()))?;
        let mut file = File::open(&path)
            .await
            .with_context(|| format!("failed to open {}", path.display()))?;
        let offset = file
            .seek(SeekFrom::End(0))
            .await
            .with_context(|| format!("failed to seek {}", path.display()))?;

        Ok(Self {
            path,
            identity,
            file,
            offset,
            pending: Vec::new(),
            generation: 0,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub async fn read_new_lines(&mut self) -> Result<Vec<String>> {
        let current_identity = Handle::from_path(&self.path)
            .with_context(|| format!("failed to identify {}", self.path.display()))?;

        if current_identity != self.identity {
            self.reopen_from_end().await?;
            return Ok(Vec::new());
        }

        let current_size = self
            .file
            .metadata()
            .await
            .with_context(|| format!("failed to stat {}", self.path.display()))?
            .len();

        if current_size < self.offset {
            self.reopen_from_end().await?;
            return Ok(Vec::new());
        }

        self.file
            .seek(SeekFrom::Start(self.offset))
            .await
            .with_context(|| format!("failed to seek {}", self.path.display()))?;

        let mut appended = Vec::new();
        let read = self
            .file
            .read_to_end(&mut appended)
            .await
            .with_context(|| format!("failed to read {}", self.path.display()))?;

        if read == 0 {
            return Ok(Vec::new());
        }

        self.offset += read as u64;
        self.pending.extend_from_slice(&appended);

        let lines = self.take_complete_lines();

        if !lines.is_empty() {
            diagnostics::mark_log_activity();
        }

        Ok(lines)
    }

    async fn reopen_from_end(&mut self) -> Result<()> {
        let identity = Handle::from_path(&self.path)
            .with_context(|| format!("failed to identify {}", self.path.display()))?;
        let mut file = File::open(&self.path)
            .await
            .with_context(|| format!("failed to reopen {}", self.path.display()))?;
        let offset = file
            .seek(SeekFrom::End(0))
            .await
            .with_context(|| format!("failed to seek {}", self.path.display()))?;

        self.identity = identity;
        self.file = file;
        self.offset = offset;
        self.pending.clear();
        self.generation = self.generation.saturating_add(1);

        Ok(())
    }

    fn take_complete_lines(&mut self) -> Vec<String> {
        let mut lines = Vec::new();
        let mut consumed = 0;

        for (index, byte) in self.pending.iter().enumerate() {
            if *byte != b'\n' {
                continue;
            }

            let line = String::from_utf8_lossy(&self.pending[consumed..index]);
            lines.push(line.trim_end_matches('\r').to_owned());
            consumed = index + 1;
        }

        if consumed > 0 {
            self.pending.drain(..consumed);
        }

        lines
    }
}

pub async fn scan_existing_log_lines<F>(path: impl AsRef<Path>, mut consume: F) -> Result<()>
where
    F: FnMut(&str),
{
    let path = path.as_ref();
    let file = File::open(path)
        .await
        .with_context(|| format!("failed to open existing log context {}", path.display()))?;
    let mut reader = BufReader::new(file);
    let mut buffer = Vec::new();

    loop {
        buffer.clear();

        let read = reader
            .read_until(b'\n', &mut buffer)
            .await
            .with_context(|| format!("failed to scan existing log context {}", path.display()))?;

        if read == 0 {
            return Ok(());
        }

        if buffer.last() == Some(&b'\n') {
            buffer.pop();
        }

        if buffer.last() == Some(&b'\r') {
            buffer.pop();
        }

        let line = String::from_utf8_lossy(&buffer);
        consume(&line);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::fs::{self, OpenOptions};
    use tokio::io::AsyncWriteExt;
    use uuid::Uuid;

    #[tokio::test]
    async fn starts_at_end_and_only_reads_new_complete_lines() {
        let directory = std::env::temp_dir().join(format!("mnemos-tail-{}", Uuid::now_v7()));
        let path = directory.join("latest.log");

        fs::create_dir_all(&directory).await.unwrap();
        fs::write(&path, b"historical\n").await.unwrap();

        let mut tailer = LogTailer::open_from_end(&path).await.unwrap();
        let mut writer = OpenOptions::new().append(true).open(&path).await.unwrap();

        writer.write_all(b"fresh-one\nfresh").await.unwrap();
        writer.flush().await.unwrap();

        assert_eq!(tailer.read_new_lines().await.unwrap(), vec!["fresh-one"]);
        assert_eq!(tailer.generation(), 0);

        writer.write_all(b"-two\n").await.unwrap();
        writer.flush().await.unwrap();

        assert_eq!(tailer.read_new_lines().await.unwrap(), vec!["fresh-two"]);
        assert_eq!(tailer.generation(), 0);

        let _ = fs::remove_dir_all(directory).await;
    }

    #[tokio::test]
    async fn scans_existing_context_without_replaying_it_through_tailer() {
        let directory = std::env::temp_dir().join(format!("mnemos-tail-{}", Uuid::now_v7()));
        let path = directory.join("latest.log");

        fs::create_dir_all(&directory).await.unwrap();
        fs::write(&path, b"historical-one\r\nhistorical-two\n")
            .await
            .unwrap();

        let mut tailer = LogTailer::open_from_end(&path).await.unwrap();
        let mut context = Vec::new();

        scan_existing_log_lines(&path, |line| context.push(line.to_owned()))
            .await
            .unwrap();

        assert_eq!(context, vec!["historical-one", "historical-two"]);
        assert!(tailer.read_new_lines().await.unwrap().is_empty());

        let mut writer = OpenOptions::new().append(true).open(&path).await.unwrap();
        writer.write_all(b"fresh\n").await.unwrap();
        writer.flush().await.unwrap();

        assert_eq!(tailer.read_new_lines().await.unwrap(), vec!["fresh"]);

        let _ = fs::remove_dir_all(directory).await;
    }

    #[tokio::test]
    async fn truncation_reopens_at_new_end_without_replaying_rewritten_content() {
        let directory = std::env::temp_dir().join(format!("mnemos-tail-{}", Uuid::now_v7()));
        let path = directory.join("latest.log");

        fs::create_dir_all(&directory).await.unwrap();
        fs::write(&path, b"historical-content-that-is-long\n")
            .await
            .unwrap();

        let mut tailer = LogTailer::open_from_end(&path).await.unwrap();

        fs::write(&path, b"new-session\n").await.unwrap();

        assert!(tailer.read_new_lines().await.unwrap().is_empty());
        assert_eq!(tailer.generation(), 1);

        let mut writer = OpenOptions::new().append(true).open(&path).await.unwrap();
        writer.write_all(b"fresh-event\n").await.unwrap();
        writer.flush().await.unwrap();

        assert_eq!(tailer.read_new_lines().await.unwrap(), vec!["fresh-event"]);

        let _ = fs::remove_dir_all(directory).await;
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn file_replacement_reopens_at_replacement_end() {
        let directory = std::env::temp_dir().join(format!("mnemos-tail-{}", Uuid::now_v7()));
        let path = directory.join("latest.log");
        let previous = directory.join("previous.log");

        fs::create_dir_all(&directory).await.unwrap();
        fs::write(&path, b"old-session\n").await.unwrap();

        let mut tailer = LogTailer::open_from_end(&path).await.unwrap();

        fs::rename(&path, &previous).await.unwrap();
        fs::write(&path, b"replacement-history\n").await.unwrap();

        assert!(tailer.read_new_lines().await.unwrap().is_empty());
        assert_eq!(tailer.generation(), 1);

        let mut writer = OpenOptions::new().append(true).open(&path).await.unwrap();
        writer.write_all(b"replacement-fresh\n").await.unwrap();
        writer.flush().await.unwrap();

        assert_eq!(
            tailer.read_new_lines().await.unwrap(),
            vec!["replacement-fresh"]
        );

        let _ = fs::remove_dir_all(directory).await;
    }
}
