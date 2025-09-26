use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::{mpsc, Mutex};
use tokio::time::{sleep, Duration};
use tracing::{debug, error, info, info_span, warn};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileRequest {
    pub file_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileResponse {
    pub file_data: Vec<u8>,
    pub file_name: String,
    pub file_size: u64,
}

// Simplified file transfer service without complex libp2p request-response
// This provides basic file storage and retrieval functionality

#[derive(Debug)]
pub enum FileTransferCommand {
    UploadFile {
        file_path: String,
        file_name: String,
    },
    DownloadFile {
        file_hash: String,
        output_path: String,
    },
    GetStoredFiles,
}

#[derive(Debug, Clone)]
pub enum FileTransferEvent {
    FileUploaded {
        file_hash: String,
        file_name: String,
    },
    FileDownloaded {
        file_path: String,
    },
    FileNotFound {
        file_hash: String,
    },
    Error {
        message: String,
    },
    DownloadAttempt(DownloadAttemptSnapshot),
}

const MAX_DOWNLOAD_ATTEMPTS: u32 = 3;
const BASE_BACKOFF_MS: u64 = 250;
const MAX_BACKOFF_MS: u64 = 1_500;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AttemptStatus {
    Retrying,
    Success,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadAttemptSnapshot {
    pub file_hash: String,
    pub attempt: u32,
    pub max_attempts: u32,
    pub status: AttemptStatus,
    pub duration_ms: u64,
    pub timestamp: u64,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DownloadMetricsSnapshot {
    pub total_success: u64,
    pub total_failures: u64,
    pub total_retries: u64,
    pub recent_attempts: Vec<DownloadAttemptSnapshot>,
}

#[derive(Debug, Default, Clone)]
struct DownloadMetrics {
    total_success: u64,
    total_failures: u64,
    total_retries: u64,
    recent_attempts: VecDeque<DownloadAttemptSnapshot>,
}

impl DownloadMetrics {
    fn record_attempt(&mut self, snapshot: DownloadAttemptSnapshot) {
        match snapshot.status {
            AttemptStatus::Retrying => {
                self.total_retries = self.total_retries.saturating_add(1);
            }
            AttemptStatus::Success => {
                self.total_success = self.total_success.saturating_add(1);
            }
            AttemptStatus::Failed => {
                self.total_failures = self.total_failures.saturating_add(1);
            }
        }

        self.recent_attempts.push_front(snapshot);
        while self.recent_attempts.len() > 20 {
            self.recent_attempts.pop_back();
        }
    }

    fn snapshot(&self) -> DownloadMetricsSnapshot {
        DownloadMetricsSnapshot {
            total_success: self.total_success,
            total_failures: self.total_failures,
            total_retries: self.total_retries,
            recent_attempts: self.recent_attempts.iter().cloned().collect(),
        }
    }
}

#[cfg(test)]
use std::sync::atomic::{AtomicU32, Ordering};

#[cfg(test)]
static LAST_DOWNLOAD_ATTEMPTS: AtomicU32 = AtomicU32::new(0);

#[cfg(test)]
static FAIL_WRITE_BEFORE_SUCCESS: AtomicU32 = AtomicU32::new(0);

pub struct FileTransferService {
    cmd_tx: mpsc::Sender<FileTransferCommand>,
    event_rx: Arc<Mutex<mpsc::Receiver<FileTransferEvent>>>,
    stored_files: Arc<Mutex<HashMap<String, (String, Vec<u8>)>>>, // hash -> (name, data)
    download_metrics: Arc<Mutex<DownloadMetrics>>,
}

impl FileTransferService {
    fn backoff_delay(attempt: u32) -> Duration {
        if attempt <= 1 {
            return Duration::from_millis(0);
        }

        let shift = (attempt - 1).min(4);
        let multiplier = 1u64 << shift;
        let delay = BASE_BACKOFF_MS.saturating_mul(multiplier);
        Duration::from_millis(delay.min(MAX_BACKOFF_MS))
    }

    async fn download_with_retries(
        file_hash: &str,
        output_path: &str,
        stored_files: &Arc<Mutex<HashMap<String, (String, Vec<u8>)>>>,
        event_tx: mpsc::Sender<FileTransferEvent>,
        download_metrics: Arc<Mutex<DownloadMetrics>>,
    ) -> Result<(), String> {
        let mut attempt = 0u32;
        let mut last_error: Option<String> = None;

        while attempt < MAX_DOWNLOAD_ATTEMPTS {
            attempt += 1;
            let span = info_span!(
                "download_attempt",
                module = "file_transfer",
                hash = %file_hash,
                attempt,
                max_attempts = MAX_DOWNLOAD_ATTEMPTS
            );
            let start = Instant::now();

            if attempt > 1 {
                let delay = Self::backoff_delay(attempt);
                span.in_scope(|| debug!(?delay, "waiting before retry"));
                if delay > Duration::from_millis(0) {
                    sleep(delay).await;
                }
            }

            let result = {
                let _guard = span.enter();
                Self::handle_download_file(file_hash, output_path, stored_files).await
            };

            match result {
                Ok(()) => {
                    let duration_ms = start.elapsed().as_millis() as u64;
                    span.in_scope(|| info!(duration_ms = duration_ms, "download_succeeded"));
                    let snapshot = DownloadAttemptSnapshot {
                        file_hash: file_hash.to_string(),
                        attempt,
                        max_attempts: MAX_DOWNLOAD_ATTEMPTS,
                        status: AttemptStatus::Success,
                        duration_ms,
                        timestamp: SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs(),
                    };
                    Self::emit_attempt(event_tx.clone(), download_metrics.clone(), snapshot).await;
                    #[cfg(test)]
                    {
                        LAST_DOWNLOAD_ATTEMPTS.store(attempt, Ordering::SeqCst);
                    }
                    return Ok(());
                }
                Err(err) => {
                    let duration_ms = start.elapsed().as_millis() as u64;
                    span.in_scope(|| warn!(duration_ms = duration_ms, %err, "download_failed"));
                    last_error = Some(err.clone());

                    let status = if attempt >= MAX_DOWNLOAD_ATTEMPTS {
                        AttemptStatus::Failed
                    } else {
                        AttemptStatus::Retrying
                    };

                    let snapshot = DownloadAttemptSnapshot {
                        file_hash: file_hash.to_string(),
                        attempt,
                        max_attempts: MAX_DOWNLOAD_ATTEMPTS,
                        status,
                        duration_ms,
                        timestamp: SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs(),
                    };
                    Self::emit_attempt(event_tx.clone(), download_metrics.clone(), snapshot).await;

                    if attempt >= MAX_DOWNLOAD_ATTEMPTS {
                        #[cfg(test)]
                        {
                            LAST_DOWNLOAD_ATTEMPTS.store(attempt, Ordering::SeqCst);
                        }
                        return Err(err);
                    }
                }
            }
        }

        Err(last_error.unwrap_or_else(|| "Download failed".to_string()))
    }

    async fn write_output(output_path: &str, data: &[u8]) -> Result<(), String> {
        #[cfg(test)]
        {
            let remaining = FAIL_WRITE_BEFORE_SUCCESS.load(Ordering::SeqCst);
            if remaining > 0 {
                FAIL_WRITE_BEFORE_SUCCESS.fetch_sub(1, Ordering::SeqCst);
                return Err("simulated write failure".to_string());
            }
        }

        tokio::fs::write(output_path, data)
            .await
            .map_err(|e| format!("Failed to write file: {}", e))
    }

    async fn emit_attempt(
        event_tx: mpsc::Sender<FileTransferEvent>,
        download_metrics: Arc<Mutex<DownloadMetrics>>,
        snapshot: DownloadAttemptSnapshot,
    ) {
        {
            let mut metrics = download_metrics.lock().await;
            metrics.record_attempt(snapshot.clone());
        }

        if let Err(err) = event_tx
            .send(FileTransferEvent::DownloadAttempt(snapshot))
            .await
        {
            warn!("failed to forward download attempt event: {}", err);
        }
    }

    #[cfg(test)]
    pub(crate) fn reset_retry_counters() {
        LAST_DOWNLOAD_ATTEMPTS.store(0, Ordering::SeqCst);
        FAIL_WRITE_BEFORE_SUCCESS.store(0, Ordering::SeqCst);
    }

    #[cfg(test)]
    pub(crate) fn set_fail_write_attempts(count: u32) {
        FAIL_WRITE_BEFORE_SUCCESS.store(count, Ordering::SeqCst);
    }

    #[cfg(test)]
    pub(crate) fn last_attempts() -> u32 {
        LAST_DOWNLOAD_ATTEMPTS.load(Ordering::SeqCst)
    }

    pub async fn new() -> Result<Self, String> {
        let (cmd_tx, cmd_rx) = mpsc::channel(100);
        let (event_tx, event_rx) = mpsc::channel(100);
        let stored_files = Arc::new(Mutex::new(HashMap::new()));
        let download_metrics = Arc::new(Mutex::new(DownloadMetrics::default()));

        // Spawn the file transfer service task
        tokio::spawn(Self::run_file_transfer_service(
            cmd_rx,
            event_tx,
            stored_files.clone(),
            download_metrics.clone(),
        ));

        Ok(FileTransferService {
            cmd_tx,
            event_rx: Arc::new(Mutex::new(event_rx)),
            stored_files,
            download_metrics,
        })
    }

    async fn run_file_transfer_service(
        mut cmd_rx: mpsc::Receiver<FileTransferCommand>,
        event_tx: mpsc::Sender<FileTransferEvent>,
        stored_files: Arc<Mutex<HashMap<String, (String, Vec<u8>)>>>,
        download_metrics: Arc<Mutex<DownloadMetrics>>,
    ) {
        while let Some(cmd) = cmd_rx.recv().await {
            match cmd {
                FileTransferCommand::UploadFile {
                    file_path,
                    file_name,
                } => match Self::handle_upload_file(&file_path, &file_name, &stored_files).await {
                    Ok(file_hash) => {
                        let _ = event_tx
                            .send(FileTransferEvent::FileUploaded {
                                file_hash: file_hash.clone(),
                                file_name: file_name.clone(),
                            })
                            .await;
                        info!("File uploaded successfully: {} -> {}", file_name, file_hash);
                    }
                    Err(e) => {
                        let error_msg = format!("Upload failed: {}", e);
                        let _ = event_tx
                            .send(FileTransferEvent::Error {
                                message: error_msg.clone(),
                            })
                            .await;
                        error!("File upload failed: {}", error_msg);
                    }
                },
                FileTransferCommand::DownloadFile {
                    file_hash,
                    output_path,
                } => {
                    match Self::download_with_retries(
                        &file_hash,
                        &output_path,
                        &stored_files,
                        event_tx.clone(),
                        download_metrics.clone(),
                    )
                    .await
                    {
                        Ok(()) => {
                            let _ = event_tx
                                .send(FileTransferEvent::FileDownloaded {
                                    file_path: output_path.clone(),
                                })
                                .await;
                            info!(
                                "File downloaded successfully: {} -> {}",
                                file_hash, output_path
                            );
                        }
                        Err(e) => {
                            let error_msg = format!("Download failed: {}", e);
                            let _ = event_tx
                                .send(FileTransferEvent::Error {
                                    message: error_msg.clone(),
                                })
                                .await;
                            error!("File download failed: {}", error_msg);
                        }
                    }
                }
                FileTransferCommand::GetStoredFiles => {
                    // This could be used to list available files
                    debug!("GetStoredFiles command received");
                }
            }
        }
    }

    async fn handle_upload_file(
        file_path: &str,
        file_name: &str,
        stored_files: &Arc<Mutex<HashMap<String, (String, Vec<u8>)>>>,
    ) -> Result<String, String> {
        // Read the file
        let file_data = tokio::fs::read(file_path)
            .await
            .map_err(|e| format!("Failed to read file: {}", e))?;

        // Calculate file hash
        let file_hash = Self::calculate_file_hash(&file_data);

        // Store the file in memory (in a real implementation, this would be persistent storage)
        {
            let mut files = stored_files.lock().await;
            files.insert(file_hash.clone(), (file_name.to_string(), file_data));
        }

        Ok(file_hash)
    }

    async fn handle_download_file(
        file_hash: &str,
        output_path: &str,
        stored_files: &Arc<Mutex<HashMap<String, (String, Vec<u8>)>>>,
    ) -> Result<(), String> {
        // Check if we have the file locally
        let (file_name, file_data) = {
            let files = stored_files.lock().await;
            files
                .get(file_hash)
                .ok_or_else(|| "File not found locally".to_string())?
                .clone()
        };

        // Write the file to the output path
        Self::write_output(output_path, &file_data).await?;

        info!("File downloaded: {} -> {}", file_name, output_path);
        Ok(())
    }

    pub fn calculate_file_hash(data: &[u8]) -> String {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(data);
        format!("{:x}", hasher.finalize())
    }

    pub async fn upload_file(&self, file_path: String, file_name: String) -> Result<(), String> {
        self.cmd_tx
            .send(FileTransferCommand::UploadFile {
                file_path,
                file_name,
            })
            .await
            .map_err(|e| e.to_string())
    }

    pub async fn download_file(
        &self,
        file_hash: String,
        output_path: String,
    ) -> Result<(), String> {
        self.cmd_tx
            .send(FileTransferCommand::DownloadFile {
                file_hash,
                output_path,
            })
            .await
            .map_err(|e| e.to_string())
    }

    pub async fn get_stored_files(&self) -> Result<Vec<(String, String)>, String> {
        let files = self.stored_files.lock().await;
        Ok(files
            .iter()
            .map(|(hash, (name, _))| (hash.clone(), name.clone()))
            .collect())
    }

    pub async fn drain_events(&self, max: usize) -> Vec<FileTransferEvent> {
        let mut events = Vec::new();
        let mut event_rx = self.event_rx.lock().await;

        for _ in 0..max {
            match event_rx.try_recv() {
                Ok(event) => events.push(event),
                Err(_) => break,
            }
        }

        events
    }

    pub async fn store_file_data(&self, file_hash: String, file_name: String, file_data: Vec<u8>) {
        let mut stored_files = self.stored_files.lock().await;
        stored_files.insert(file_hash, (file_name, file_data));
    }

    pub async fn download_metrics_snapshot(&self) -> DownloadMetricsSnapshot {
        let metrics = self.download_metrics.lock().await;
        metrics.snapshot()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tempfile::tempdir;
    use tokio::sync::{mpsc, Mutex};

    #[tokio::test]
    async fn download_retries_then_succeeds() {
        FileTransferService::reset_retry_counters();
        FileTransferService::set_fail_write_attempts(2);

        let stored_files = Arc::new(Mutex::new(HashMap::new()));
        {
            let mut guard = stored_files.lock().await;
            guard.insert(
                "test-hash".to_string(),
                ("example.txt".to_string(), b"hello world".to_vec()),
            );
        }

        let temp_dir = tempdir().expect("temp dir");
        let output_path = temp_dir.path().join("downloaded.txt");
        let output_str = output_path.to_string_lossy().to_string();

        let (event_tx, mut event_rx) = mpsc::channel(16);
        let metrics = Arc::new(Mutex::new(DownloadMetrics::default()));

        let result = FileTransferService::download_with_retries(
            "test-hash",
            &output_str,
            &stored_files,
            event_tx.clone(),
            metrics.clone(),
        )
        .await;

        assert!(result.is_ok(), "expected download to succeed: {result:?}");

        let written = tokio::fs::read(&output_path).await.expect("file read");
        assert_eq!(written, b"hello world");
        let metrics_snapshot = metrics.lock().await.snapshot();
        assert_eq!(metrics_snapshot.total_success, 1);
        assert_eq!(metrics_snapshot.total_retries, 2);
        assert_eq!(metrics_snapshot.recent_attempts.len(), 3);

        // Ensure we received attempt events
        let mut statuses = Vec::new();
        while let Ok(event) = event_rx.try_recv() {
            if let FileTransferEvent::DownloadAttempt(snapshot) = event {
                statuses.push(snapshot.status);
            }
        }
        assert!(statuses.contains(&AttemptStatus::Retrying));
        assert!(statuses.contains(&AttemptStatus::Success));

        let snapshot = metrics.lock().await.snapshot();
        assert_eq!(snapshot.total_success, 1);
        assert_eq!(snapshot.total_failures, 0);
        assert_eq!(snapshot.total_retries, 2);
    }

    #[tokio::test]
    async fn download_fails_after_max_attempts_for_missing_file() {
        FileTransferService::reset_retry_counters();
        FileTransferService::set_fail_write_attempts(0);

        let stored_files = Arc::new(Mutex::new(HashMap::new()));

        let temp_dir = tempdir().expect("temp dir");
        let output_path = temp_dir.path().join("missing.txt");
        let output_str = output_path.to_string_lossy().to_string();

        let (event_tx, mut event_rx) = mpsc::channel(16);
        let metrics = Arc::new(Mutex::new(DownloadMetrics::default()));

        let result = FileTransferService::download_with_retries(
            "missing-hash",
            &output_str,
            &stored_files,
            event_tx.clone(),
            metrics.clone(),
        )
        .await;

        assert!(result.is_err(), "expected download to fail");
        assert_eq!(FileTransferService::last_attempts(), MAX_DOWNLOAD_ATTEMPTS);

        let mut failure_seen = false;
        while let Ok(event) = event_rx.try_recv() {
            if let FileTransferEvent::DownloadAttempt(snapshot) = event {
                if matches!(snapshot.status, AttemptStatus::Failed) {
                    failure_seen = true;
                }
            }
        }
        assert!(failure_seen, "expected a failed attempt event");

        let snapshot = metrics.lock().await.snapshot();
        assert_eq!(snapshot.total_success, 0);
        assert_eq!(snapshot.total_failures, 1);
        assert_eq!(
            snapshot.total_retries,
            MAX_DOWNLOAD_ATTEMPTS.saturating_sub(1) as u64
        );
    }
}
