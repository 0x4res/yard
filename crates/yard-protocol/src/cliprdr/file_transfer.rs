//! File transfer state machine for clipboard file operations.
//!
//! Story 5.4: Implements the staged file download process:
//! 1. Receive file list (metadata) from server
//! 2. Request file size for each file
//! 3. Request file content in chunks
//! 4. Write to temporary staging directory
//! 5. Notify main thread when file is ready

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

use tracing::{debug, info, warn};

/// Default chunk size for file content requests (64KB).
pub const DEFAULT_CHUNK_SIZE: u32 = 64 * 1024;

/// Maximum file size we'll transfer (1GB to prevent abuse).
pub const MAX_FILE_SIZE: u64 = 1024 * 1024 * 1024;

/// State of a single file transfer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileTransferState {
    /// Waiting to start transfer.
    Pending,
    /// Waiting for file size response.
    AwaitingSize {
        /// Stream ID for the size request.
        stream_id: u32,
    },
    /// Downloading file content.
    Downloading {
        /// Total file size.
        total_size: u64,
        /// Bytes received so far.
        bytes_received: u64,
        /// Current stream ID for content request.
        current_stream_id: u32,
    },
    /// Transfer completed successfully.
    Completed {
        /// Path to the downloaded file.
        path: PathBuf,
    },
    /// Transfer failed.
    Failed {
        /// Error message.
        error: String,
    },
}

/// Information about a pending file transfer.
#[derive(Debug)]
pub struct PendingFileTransfer {
    /// File index in the clipboard file list.
    pub file_index: u32,
    /// Original file name from the server.
    pub file_name: String,
    /// Known file size (from FileGroupDescriptorW, may differ from actual).
    pub declared_size: Option<u64>,
    /// True if this is a directory.
    pub is_directory: bool,
    /// Current transfer state.
    pub state: FileTransferState,
    /// Temporary file handle for writing.
    temp_file: Option<File>,
    /// Path to the temporary file.
    temp_path: Option<PathBuf>,
}

impl PendingFileTransfer {
    /// Creates a new pending file transfer.
    pub fn new(file_index: u32, file_name: String, declared_size: Option<u64>, is_directory: bool) -> Self {
        Self {
            file_index,
            file_name,
            declared_size,
            is_directory,
            state: FileTransferState::Pending,
            temp_file: None,
            temp_path: None,
        }
    }

    /// Returns the final file path (in staging directory).
    pub fn final_path(&self) -> Option<&Path> {
        if let FileTransferState::Completed { path } = &self.state {
            Some(path)
        } else {
            None
        }
    }
}

/// Manages file transfers for clipboard operations.
#[derive(Debug)]
pub struct FileTransferManager {
    /// Staging directory for downloaded files.
    staging_dir: PathBuf,
    /// Map of stream ID to file index for tracking responses.
    stream_to_file: HashMap<u32, u32>,
    /// Pending file transfers by file index.
    transfers: HashMap<u32, PendingFileTransfer>,
    /// Next stream ID to use.
    next_stream_id: u32,
    /// Chunk size for content requests.
    chunk_size: u32,
}

impl FileTransferManager {
    /// Creates a new file transfer manager with a staging directory.
    ///
    /// Creates the staging directory if it doesn't exist.
    pub fn new(staging_dir: PathBuf) -> std::io::Result<Self> {
        // Create staging directory with secure permissions
        fs::create_dir_all(&staging_dir)?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let permissions = fs::Permissions::from_mode(0o700);
            fs::set_permissions(&staging_dir, permissions)?;
        }

        info!("File transfer staging directory: {:?}", staging_dir);

        Ok(Self {
            staging_dir,
            stream_to_file: HashMap::new(),
            transfers: HashMap::new(),
            next_stream_id: 1,
            chunk_size: DEFAULT_CHUNK_SIZE,
        })
    }

    /// Creates a new file transfer manager with a default staging directory.
    pub fn with_default_staging_dir() -> std::io::Result<Self> {
        let staging_dir = default_staging_dir()?;
        Self::new(staging_dir)
    }

    /// Returns the staging directory path.
    pub fn staging_dir(&self) -> &Path {
        &self.staging_dir
    }

    /// Allocates a new stream ID.
    fn next_stream_id(&mut self) -> u32 {
        let id = self.next_stream_id;
        self.next_stream_id = self.next_stream_id.wrapping_add(1);
        id
    }

    /// Adds files from a clipboard file list to the transfer queue.
    pub fn add_files(&mut self, files: &[(String, Option<u64>, bool)]) {
        // Clear previous transfers
        self.clear();

        for (index, (name, size, is_dir)) in files.iter().enumerate() {
            let file_index = index as u32;
            let transfer = PendingFileTransfer::new(
                file_index,
                name.clone(),
                *size,
                *is_dir,
            );
            self.transfers.insert(file_index, transfer);
        }

        debug!("Added {} files to transfer queue", files.len());
    }

    /// Starts a file transfer by requesting its size.
    ///
    /// Returns the stream ID and file index, or None if no pending transfers.
    pub fn start_next_transfer(&mut self) -> Option<(u32, u32)> {
        // Find first pending transfer
        let file_index = self.transfers.iter()
            .find(|(_, t)| t.state == FileTransferState::Pending && !t.is_directory)
            .map(|(&idx, _)| idx)?;

        let stream_id = self.next_stream_id();

        if let Some(transfer) = self.transfers.get_mut(&file_index) {
            transfer.state = FileTransferState::AwaitingSize { stream_id };
            self.stream_to_file.insert(stream_id, file_index);
            debug!(
                "Starting transfer for file {}: {} (stream_id={})",
                file_index, transfer.file_name, stream_id
            );
            Some((stream_id, file_index))
        } else {
            None
        }
    }

    /// Handles a file size response.
    ///
    /// Returns the next content request (stream_id, file_index, offset, length) if ready.
    pub fn handle_size_response(
        &mut self,
        stream_id: u32,
        size: u64,
    ) -> Result<Option<(u32, u32, u64, u32)>, String> {
        let file_index = self.stream_to_file.remove(&stream_id)
            .ok_or_else(|| format!("Unknown stream_id: {}", stream_id))?;

        // Validate state first
        {
            let transfer = self.transfers.get(&file_index)
                .ok_or_else(|| format!("Unknown file_index: {}", file_index))?;

            if !matches!(transfer.state, FileTransferState::AwaitingSize { .. }) {
                return Err(format!(
                    "Unexpected size response for file {} in state {:?}",
                    file_index, transfer.state
                ));
            }
        }

        // Check file size limit
        if size > MAX_FILE_SIZE {
            let transfer = self.transfers.get_mut(&file_index).unwrap();
            transfer.state = FileTransferState::Failed {
                error: format!("File too large: {} bytes (max {})", size, MAX_FILE_SIZE),
            };
            return Ok(None);
        }

        // Handle empty files
        if size == 0 {
            let transfer = self.transfers.get_mut(&file_index).unwrap();
            let final_path = self.staging_dir.join(&transfer.file_name);
            if let Err(e) = File::create(&final_path) {
                transfer.state = FileTransferState::Failed {
                    error: format!("Failed to create file: {}", e),
                };
                return Ok(None);
            }
            transfer.state = FileTransferState::Completed { path: final_path };
            debug!("Empty file {} completed", transfer.file_name);
            return Ok(None);
        }

        // Get file name for temp path
        let file_name = {
            let transfer = self.transfers.get(&file_index).unwrap();
            transfer.file_name.clone()
        };

        // Create temporary file
        let temp_path = self.staging_dir.join(format!(".{}.part", file_name));
        let temp_file = File::create(&temp_path).map_err(|e| {
            format!("Failed to create temp file: {}", e)
        })?;

        // Get new stream ID before borrowing transfer mutably
        let new_stream_id = self.next_stream_id();
        let chunk_len = std::cmp::min(self.chunk_size as u64, size) as u32;

        // Now update the transfer
        let transfer = self.transfers.get_mut(&file_index).unwrap();
        transfer.temp_file = Some(temp_file);
        transfer.temp_path = Some(temp_path);

        transfer.state = FileTransferState::Downloading {
            total_size: size,
            bytes_received: 0,
            current_stream_id: new_stream_id,
        };

        self.stream_to_file.insert(new_stream_id, file_index);

        debug!(
            "File {} size: {} bytes, requesting first chunk (stream_id={})",
            file_name, size, new_stream_id
        );

        Ok(Some((new_stream_id, file_index, 0, chunk_len)))
    }

    /// Handles a file content response.
    ///
    /// Returns the next content request if more data needed, or None if complete.
    pub fn handle_content_response(
        &mut self,
        stream_id: u32,
        data: &[u8],
    ) -> Result<Option<(u32, u32, u64, u32)>, String> {
        let file_index = self.stream_to_file.remove(&stream_id)
            .ok_or_else(|| format!("Unknown stream_id: {}", stream_id))?;

        // Get state info first
        let (total_size, bytes_received, file_name) = {
            let transfer = self.transfers.get(&file_index)
                .ok_or_else(|| format!("Unknown file_index: {}", file_index))?;

            match &transfer.state {
                FileTransferState::Downloading { total_size, bytes_received, .. } => {
                    (*total_size, *bytes_received, transfer.file_name.clone())
                }
                _ => {
                    return Err(format!(
                        "Unexpected content response for file {} in state {:?}",
                        file_index, transfer.state
                    ));
                }
            }
        };

        // Write data to temp file
        {
            let transfer = self.transfers.get_mut(&file_index).unwrap();
            if let Some(ref mut file) = transfer.temp_file {
                file.write_all(data).map_err(|e| format!("Write failed: {}", e))?;
            } else {
                return Err("No temp file handle".to_string());
            }
        }

        let new_bytes_received = bytes_received + data.len() as u64;

        // Check if transfer is complete
        if new_bytes_received >= total_size {
            let transfer = self.transfers.get_mut(&file_index).unwrap();
            // Close and rename file
            transfer.temp_file = None;

            if let Some(temp_path) = transfer.temp_path.take() {
                let final_path = self.staging_dir.join(&transfer.file_name);
                fs::rename(&temp_path, &final_path).map_err(|e| {
                    format!("Failed to rename temp file: {}", e)
                })?;

                transfer.state = FileTransferState::Completed { path: final_path.clone() };
                info!(
                    "File transfer complete: {} ({} bytes)",
                    transfer.file_name, new_bytes_received
                );
            }

            return Ok(None);
        }

        // Request next chunk - get stream ID before mutable borrow
        let new_stream_id = self.next_stream_id();
        let remaining = total_size - new_bytes_received;
        let chunk_len = std::cmp::min(self.chunk_size as u64, remaining) as u32;

        // Update transfer state
        let transfer = self.transfers.get_mut(&file_index).unwrap();
        transfer.state = FileTransferState::Downloading {
            total_size,
            bytes_received: new_bytes_received,
            current_stream_id: new_stream_id,
        };

        self.stream_to_file.insert(new_stream_id, file_index);

        debug!(
            "File {} progress: {}/{} bytes, requesting next chunk",
            file_name, new_bytes_received, total_size
        );

        Ok(Some((new_stream_id, file_index, new_bytes_received, chunk_len)))
    }

    /// Handles a transfer failure.
    pub fn handle_failure(&mut self, stream_id: u32) {
        if let Some(file_index) = self.stream_to_file.remove(&stream_id) {
            if let Some(transfer) = self.transfers.get_mut(&file_index) {
                // Clean up temp file
                if let Some(temp_path) = transfer.temp_path.take() {
                    let _ = fs::remove_file(&temp_path);
                }
                transfer.temp_file = None;

                transfer.state = FileTransferState::Failed {
                    error: "Server returned failure".to_string(),
                };

                warn!("File transfer failed: {}", transfer.file_name);
            }
        }
    }

    /// Returns the list of completed file paths.
    pub fn completed_files(&self) -> Vec<PathBuf> {
        self.transfers.values()
            .filter_map(|t| t.final_path().map(|p| p.to_path_buf()))
            .collect()
    }

    /// Returns the number of pending transfers (excludes directories).
    pub fn pending_count(&self) -> usize {
        self.transfers.values()
            .filter(|t| matches!(t.state, FileTransferState::Pending) && !t.is_directory)
            .count()
    }

    /// Returns the number of active transfers.
    pub fn active_count(&self) -> usize {
        self.transfers.values()
            .filter(|t| matches!(
                t.state,
                FileTransferState::AwaitingSize { .. } | FileTransferState::Downloading { .. }
            ))
            .count()
    }

    /// Returns the number of completed transfers.
    pub fn completed_count(&self) -> usize {
        self.transfers.values()
            .filter(|t| matches!(t.state, FileTransferState::Completed { .. }))
            .count()
    }

    /// Clears all transfers and removes temporary files.
    pub fn clear(&mut self) {
        // Clean up any temp files
        for transfer in self.transfers.values_mut() {
            if let Some(temp_path) = transfer.temp_path.take() {
                let _ = fs::remove_file(&temp_path);
            }
        }

        self.transfers.clear();
        self.stream_to_file.clear();
    }

    /// Cleans up the staging directory.
    pub fn cleanup_staging_dir(&self) -> std::io::Result<()> {
        if self.staging_dir.exists() {
            fs::remove_dir_all(&self.staging_dir)?;
        }
        Ok(())
    }
}

impl Drop for FileTransferManager {
    fn drop(&mut self) {
        // Clean up temp files but keep staging directory
        self.clear();
    }
}

/// Returns the default staging directory for clipboard files.
pub fn default_staging_dir() -> std::io::Result<PathBuf> {
    // Try XDG_RUNTIME_DIR first (secure, user-specific)
    if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
        return Ok(PathBuf::from(runtime_dir).join("yard").join("clipboard"));
    }

    // Fall back to temp directory with PID
    let pid = std::process::id();
    let temp_dir = std::env::temp_dir();
    Ok(temp_dir.join(format!("yard-clipboard-{}", pid)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn create_test_manager() -> FileTransferManager {
        use std::time::{SystemTime, UNIX_EPOCH};
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_dir = std::env::temp_dir()
            .join(format!("yard-test-{}-{}", std::process::id(), timestamp));
        FileTransferManager::new(temp_dir).unwrap()
    }

    #[test]
    fn test_file_transfer_state() {
        assert_eq!(FileTransferState::Pending, FileTransferState::Pending);

        let state = FileTransferState::Downloading {
            total_size: 1000,
            bytes_received: 500,
            current_stream_id: 1,
        };

        if let FileTransferState::Downloading { total_size, bytes_received, .. } = state {
            assert_eq!(total_size, 1000);
            assert_eq!(bytes_received, 500);
        }
    }

    #[test]
    fn test_pending_file_transfer() {
        let transfer = PendingFileTransfer::new(0, "test.txt".to_string(), Some(100), false);
        assert_eq!(transfer.file_index, 0);
        assert_eq!(transfer.file_name, "test.txt");
        assert_eq!(transfer.declared_size, Some(100));
        assert!(!transfer.is_directory);
        assert_eq!(transfer.state, FileTransferState::Pending);
        assert!(transfer.final_path().is_none());
    }

    #[test]
    fn test_manager_creation() {
        let manager = create_test_manager();
        assert!(manager.staging_dir().exists());
        assert_eq!(manager.pending_count(), 0);
        assert_eq!(manager.active_count(), 0);
        let _ = manager.cleanup_staging_dir();
    }

    #[test]
    fn test_add_files() {
        let mut manager = create_test_manager();

        let files = vec![
            ("file1.txt".to_string(), Some(100u64), false),
            ("file2.pdf".to_string(), Some(500u64), false),
            ("folder".to_string(), None, true),
        ];

        manager.add_files(&files);
        assert_eq!(manager.pending_count(), 2); // Directories are not counted as pending
    }

    #[test]
    fn test_start_transfer() {
        let mut manager = create_test_manager();

        let files = vec![
            ("test.txt".to_string(), Some(100u64), false),
        ];
        manager.add_files(&files);

        let result = manager.start_next_transfer();
        assert!(result.is_some());

        let (stream_id, file_index) = result.unwrap();
        assert_eq!(file_index, 0);
        assert!(stream_id > 0);

        // Verify state changed
        assert_eq!(manager.pending_count(), 0);
        assert_eq!(manager.active_count(), 1);
    }

    #[test]
    fn test_empty_file_transfer() {
        let mut manager = create_test_manager();

        let files = vec![
            ("empty.txt".to_string(), Some(0u64), false),
        ];
        manager.add_files(&files);

        let (stream_id, _) = manager.start_next_transfer().unwrap();

        // Handle size response with 0 bytes
        let result = manager.handle_size_response(stream_id, 0);
        assert!(result.is_ok());
        assert!(result.unwrap().is_none()); // No more requests needed

        // File should be completed
        assert_eq!(manager.completed_count(), 1);

        let completed = manager.completed_files();
        assert_eq!(completed.len(), 1);
        assert!(completed[0].exists());
    }

    #[test]
    fn test_file_too_large() {
        let mut manager = create_test_manager();

        let files = vec![
            ("huge.bin".to_string(), None, false),
        ];
        manager.add_files(&files);

        let (stream_id, _) = manager.start_next_transfer().unwrap();

        // Handle size response with file too large
        let result = manager.handle_size_response(stream_id, MAX_FILE_SIZE + 1);
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());

        // Transfer should be marked as failed
        assert_eq!(manager.completed_count(), 0);
    }

    #[test]
    fn test_full_transfer() {
        let mut manager = create_test_manager();

        let files = vec![
            ("hello.txt".to_string(), Some(5u64), false),
        ];
        manager.add_files(&files);

        // Start transfer
        let (stream_id, file_index) = manager.start_next_transfer().unwrap();

        // Handle size response
        let result = manager.handle_size_response(stream_id, 5).unwrap();
        assert!(result.is_some());

        let (content_stream_id, _, offset, length) = result.unwrap();
        assert_eq!(offset, 0);
        assert_eq!(length, 5);

        // Handle content response
        let result = manager.handle_content_response(content_stream_id, b"Hello").unwrap();
        assert!(result.is_none()); // Transfer complete

        // Verify file
        assert_eq!(manager.completed_count(), 1);
        let completed = manager.completed_files();
        assert_eq!(completed.len(), 1);

        let mut content = String::new();
        File::open(&completed[0]).unwrap().read_to_string(&mut content).unwrap();
        assert_eq!(content, "Hello");
    }

    #[test]
    fn test_chunked_transfer() {
        let mut manager = create_test_manager();
        manager.chunk_size = 3; // Small chunks for testing

        let files = vec![
            ("chunks.txt".to_string(), Some(10u64), false),
        ];
        manager.add_files(&files);

        let (stream_id, _) = manager.start_next_transfer().unwrap();

        // Handle size
        let result = manager.handle_size_response(stream_id, 10).unwrap().unwrap();
        let (stream_id, _, offset, length) = result;
        assert_eq!(offset, 0);
        assert_eq!(length, 3);

        // First chunk
        let result = manager.handle_content_response(stream_id, b"ABC").unwrap().unwrap();
        let (stream_id, _, offset, length) = result;
        assert_eq!(offset, 3);
        assert_eq!(length, 3);

        // Second chunk
        let result = manager.handle_content_response(stream_id, b"DEF").unwrap().unwrap();
        let (stream_id, _, offset, length) = result;
        assert_eq!(offset, 6);
        assert_eq!(length, 3);

        // Third chunk
        let result = manager.handle_content_response(stream_id, b"GHI").unwrap().unwrap();
        let (stream_id, _, offset, length) = result;
        assert_eq!(offset, 9);
        assert_eq!(length, 1);

        // Final chunk
        let result = manager.handle_content_response(stream_id, b"J").unwrap();
        assert!(result.is_none());

        // Verify
        let completed = manager.completed_files();
        let mut content = String::new();
        File::open(&completed[0]).unwrap().read_to_string(&mut content).unwrap();
        assert_eq!(content, "ABCDEFGHIJ");
    }

    #[test]
    fn test_handle_failure() {
        let mut manager = create_test_manager();

        let files = vec![
            ("fail.txt".to_string(), Some(100u64), false),
        ];
        manager.add_files(&files);

        let (stream_id, _) = manager.start_next_transfer().unwrap();

        manager.handle_failure(stream_id);

        assert_eq!(manager.active_count(), 0);
        assert_eq!(manager.completed_count(), 0);
    }

    #[test]
    fn test_clear() {
        let mut manager = create_test_manager();

        let files = vec![
            ("test1.txt".to_string(), Some(100u64), false),
            ("test2.txt".to_string(), Some(200u64), false),
        ];
        manager.add_files(&files);

        manager.clear();

        assert_eq!(manager.pending_count(), 0);
        assert_eq!(manager.active_count(), 0);
    }

    #[test]
    fn test_default_staging_dir() {
        let dir = default_staging_dir().unwrap();
        assert!(dir.to_string_lossy().contains("yard"));
    }
}
