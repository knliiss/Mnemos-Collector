use std::io::SeekFrom;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncSeekExt};

#[derive(Debug)]
pub struct LogTailer {
    path: PathBuf,
    file: File,
    offset: u64,
    pending: Vec<u8>,
}

impl LogTailer {
    pub async fn open_from_end(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let mut file = File::open(&path)
            .await
            .with_context(|| format!("failed to open {}", path.display()))?;
        let offset = file
            .seek(SeekFrom::End(0))
            .await
            .with_context(|| format!("failed to seek {}", path.display()))?;

        Ok(Self {
            path,
            file,
            offset,
            pending: Vec::new(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub async fn read_new_lines(&mut self) -> Result<Vec<String>> {
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

        Ok(self.take_complete_lines())
    }

    async fn reopen_from_end(&mut self) -> Result<()> {
        let mut file = File::open(&self.path)
            .await
            .with_context(|| format!("failed to reopen {}", self.path.display()))?;
        let offset = file
            .seek(SeekFrom::End(0))
            .await
            .with_context(|| format!("failed to seek {}", self.path.display()))?;

        self.file = file;
        self.offset = offset;
        self.pending.clear();

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
