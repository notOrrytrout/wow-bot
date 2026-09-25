//! Bounded asynchronous JSONL diagnostics shared by workers and the proxy.

use serde_json::Value;
use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, SyncSender},
    },
};
use wow_domain::{LaneId, time::Millis};

const QUEUE_CAPACITY: usize = 4096;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum DiagnosticStream {
    Navigation,
    MovementHeartbeat,
    Transition,
    Network,
    ProxySession,
    ProxyMovement,
}

impl DiagnosticStream {
    pub const WORKER: [Self; 4] = [
        Self::Navigation,
        Self::MovementHeartbeat,
        Self::Transition,
        Self::Network,
    ];

    pub const PROXY: [Self; 2] = [Self::ProxySession, Self::ProxyMovement];

    pub const fn directory(self) -> &'static str {
        match self {
            Self::Navigation => "navigation",
            Self::MovementHeartbeat => "movement-heartbeats",
            Self::Transition => "transitions",
            Self::Network => "network",
            Self::ProxySession => "proxy/sessions",
            Self::ProxyMovement => "proxy/movement",
        }
    }

    pub const fn worker_env_key(self) -> Option<&'static str> {
        match self {
            Self::Navigation => Some("WOW_BOT_NAVIGATION_LOG"),
            Self::MovementHeartbeat => Some("WOW_BOT_MOVEMENT_HEARTBEAT_LOG"),
            Self::Transition => Some("WOW_BOT_TRANSITION_LOG"),
            Self::Network => Some("WOW_BOT_NETWORK_LOG"),
            Self::ProxySession | Self::ProxyMovement => None,
        }
    }

    fn flush_policy(self) -> FlushPolicy {
        match self {
            Self::Navigation | Self::MovementHeartbeat => FlushPolicy::FirstThenEvery(5),
            _ => FlushPolicy::EveryRecord,
        }
    }
}

pub fn diagnostic_path(log_dir: &Path, stream: DiagnosticStream, lane: LaneId) -> PathBuf {
    log_dir
        .join(stream.directory())
        .join(format!("{lane}.jsonl"))
}

#[derive(Clone)]
pub struct DiagnosticLogger {
    sender: Option<SyncSender<Command>>,
    dropped: Arc<AtomicU64>,
    run_id: Arc<str>,
}

impl std::fmt::Debug for DiagnosticLogger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DiagnosticLogger")
            .field("run_id", &self.run_id)
            .field("enabled", &self.sender.is_some())
            .finish()
    }
}

enum Command {
    Record {
        stream: DiagnosticStream,
        lane: LaneId,
        record_type: String,
        fields: Value,
    },
    Flush(mpsc::Sender<()>),
}

impl DiagnosticLogger {
    /// Start one writer thread for all supplied streams. File setup failures
    /// disable only the affected stream and never stop runtime work.
    pub fn new(
        run_id: impl Into<Arc<str>>,
        files: impl IntoIterator<Item = (DiagnosticStream, LaneId, PathBuf)>,
    ) -> Self {
        let run_id = run_id.into();
        let (sender, receiver) = mpsc::sync_channel(QUEUE_CAPACITY);
        let dropped = Arc::new(AtomicU64::new(0));
        let files: Vec<_> = files.into_iter().collect();
        let sender = match std::thread::Builder::new()
            .name("wow-bot-diagnostics".into())
            .spawn(move || writer_loop(receiver, files))
        {
            Ok(_) => Some(sender),
            Err(error) => {
                tracing::warn!(%error, "structured diagnostics disabled: writer thread could not start");
                None
            }
        };
        Self {
            sender,
            dropped,
            run_id,
        }
    }

    pub fn worker_from_env(run_id: impl Into<Arc<str>>, lane: LaneId) -> Self {
        let files = DiagnosticStream::WORKER.into_iter().filter_map(|stream| {
            let path = std::env::var_os(stream.worker_env_key()?)?;
            Some((stream, lane, PathBuf::from(path)))
        });
        Self::new(run_id, files)
    }

    /// Queue a record without waiting for disk I/O. A full queue drops only
    /// diagnostics and reports drop totals at powers of two.
    pub fn record(
        &self,
        stream: DiagnosticStream,
        lane: LaneId,
        record_type: impl Into<String>,
        mut fields: Value,
    ) {
        let Some(sender) = &self.sender else { return };
        if let Some(object) = fields.as_object_mut() {
            object.insert("lane".into(), Value::from(lane.get()));
            object.insert("run_id".into(), Value::String(self.run_id.to_string()));
        } else {
            return;
        }
        let command = Command::Record {
            stream,
            lane,
            record_type: record_type.into(),
            fields,
        };
        try_enqueue(sender, &self.dropped, command);
    }

    pub fn dropped_records(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// Wait for queued records to reach disk. Runtime emitters must use
    /// `record`, which never waits.
    pub fn flush(&self) {
        let Some(sender) = &self.sender else { return };
        let (done, flushed) = mpsc::channel();
        if sender.send(Command::Flush(done)).is_ok() {
            let _ = flushed.recv();
        }
    }
}

fn try_enqueue(sender: &SyncSender<Command>, dropped: &AtomicU64, command: Command) {
    if sender.try_send(command).is_err() {
        let total = dropped.fetch_add(1, Ordering::Relaxed) + 1;
        if total.is_power_of_two() {
            tracing::warn!(
                dropped_records = total,
                "structured diagnostic queue full or closed; records dropped"
            );
        }
    }
}

#[derive(Clone, Copy)]
enum FlushPolicy {
    EveryRecord,
    FirstThenEvery(u32),
}

struct JsonlWriter {
    writer: BufWriter<File>,
    sequence: u64,
    since_flush: u32,
    policy: FlushPolicy,
}

impl JsonlWriter {
    fn open(path: &Path, policy: FlushPolicy) -> std::io::Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        Ok(Self {
            writer: BufWriter::with_capacity(64 * 1024, file),
            sequence: 0,
            since_flush: 0,
            policy,
        })
    }

    fn write(&mut self, record_type: &str, mut fields: Value) -> std::io::Result<()> {
        let Some(object) = fields.as_object_mut() else {
            return Ok(());
        };
        self.sequence = self.sequence.wrapping_add(1);
        object.insert("record_type".into(), Value::String(record_type.to_owned()));
        object.insert("format_version".into(), Value::from(1));
        object.insert("unix_ms".into(), Value::from(Millis::wall_clock_now().0));
        object.insert("sequence".into(), Value::from(self.sequence));
        let mut line = serde_json::to_vec(&fields).map_err(std::io::Error::other)?;
        line.push(b'\n');
        self.writer.write_all(&line)?;
        self.since_flush = self.since_flush.saturating_add(1);
        if matches!(self.policy, FlushPolicy::EveryRecord)
            || self.sequence == 1
            || matches!(self.policy, FlushPolicy::FirstThenEvery(n) if self.since_flush >= n.max(1))
        {
            self.flush()?;
        }
        Ok(())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.writer.flush()?;
        self.since_flush = 0;
        Ok(())
    }
}

fn writer_loop(receiver: mpsc::Receiver<Command>, files: Vec<(DiagnosticStream, LaneId, PathBuf)>) {
    let mut writers = HashMap::new();
    for (stream, lane, path) in files {
        let writer = match JsonlWriter::open(&path, stream.flush_policy()) {
            Ok(writer) => Some(writer),
            Err(error) => {
                tracing::warn!(stream=?stream, lane=%lane, path=%path.display(), %error, "structured diagnostic stream disabled because its file could not be opened");
                None
            }
        };
        writers.insert((stream, lane), writer);
    }
    for command in receiver {
        match command {
            Command::Record {
                stream,
                lane,
                record_type,
                fields,
            } => {
                let Some(Some(writer)) = writers.get_mut(&(stream, lane)) else {
                    continue;
                };
                if let Err(error) = writer.write(&record_type, fields) {
                    tracing::warn!(stream=?stream, lane=%lane, %error, "structured diagnostic stream disabled after write failure");
                    writers.insert((stream, lane), None);
                }
            }
            Command::Flush(done) => {
                for ((stream, lane), writer) in &mut writers {
                    if let Some(mut active) = writer.take() {
                        match active.flush() {
                            Ok(()) => *writer = Some(active),
                            Err(error) => {
                                tracing::warn!(stream=?stream, %lane, %error, "structured diagnostic flush failed")
                            }
                        }
                    }
                }
                let _ = done.send(());
            }
        }
    }
    for ((stream, lane), writer) in &mut writers {
        if let Some(writer) = writer
            && let Err(error) = writer.flush()
        {
            tracing::warn!(stream=?stream, %lane, %error, "structured diagnostic shutdown flush failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn records_share_schema_sequence_and_flush_to_lane_specific_paths() {
        let root = test_dir("lanes");
        let lane_a = LaneId(10);
        let lane_b = LaneId(11);
        let streams: Vec<_> = DiagnosticStream::WORKER
            .into_iter()
            .chain(DiagnosticStream::PROXY)
            .collect();
        let earliest_unix_ms = Millis::wall_clock_now().0;
        let mut files: Vec<_> = streams
            .iter()
            .map(|stream| (*stream, lane_a, diagnostic_path(&root, *stream, lane_a)))
            .collect();
        files.push((
            DiagnosticStream::Network,
            lane_b,
            diagnostic_path(&root, DiagnosticStream::Network, lane_b),
        ));
        let logger = DiagnosticLogger::new("test-run", files);
        for stream in &streams {
            logger.record(*stream, lane_a, "run_started", json!({"pid": 1}));
        }
        logger.record(
            DiagnosticStream::Navigation,
            lane_a,
            "route_selected",
            json!({"nodes": 3}),
        );
        logger.record(
            DiagnosticStream::Network,
            lane_b,
            "action_sent",
            json!({"opcode": 124}),
        );
        logger.flush();
        let latest_unix_ms = Millis::wall_clock_now().0;

        let records = |path: PathBuf| -> Vec<Value> {
            std::fs::read_to_string(path)
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect()
        };
        for stream in &streams {
            let stream_records = records(diagnostic_path(&root, *stream, lane_a));
            assert_eq!(stream_records[0]["record_type"], "run_started");
            assert_eq!(stream_records[0]["sequence"], 1);
            assert_eq!(stream_records[0]["format_version"], 1);
            assert_eq!(stream_records[0]["run_id"], "test-run");
            let unix_ms = stream_records[0]["unix_ms"].as_u64().unwrap();
            assert!((earliest_unix_ms..=latest_unix_ms).contains(&unix_ms));
            assert_eq!(stream_records[0]["lane"], 10);
            if *stream == DiagnosticStream::Navigation {
                assert_eq!(stream_records[1]["sequence"], 2);
            }
        }
        assert_eq!(
            records(diagnostic_path(&root, DiagnosticStream::Network, lane_b))[0]["lane"],
            11
        );
        drop(logger);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn file_open_failure_does_not_stop_recording_to_other_streams() {
        let root = test_dir("failure");
        let blocked = root.join("file");
        std::fs::write(&blocked, b"x").unwrap();
        let lane = LaneId(1);
        let good = diagnostic_path(&root, DiagnosticStream::Network, lane);
        let logger = DiagnosticLogger::new(
            "test-run",
            [
                (
                    DiagnosticStream::Navigation,
                    lane,
                    blocked.join("cannot-open.jsonl"),
                ),
                (DiagnosticStream::Network, lane, good.clone()),
            ],
        );
        logger.record(DiagnosticStream::Navigation, lane, "ignored", json!({}));
        logger.record(
            DiagnosticStream::Network,
            lane,
            "still_works",
            json!({"metadata_only": true}),
        );
        logger.flush();
        let written = std::fs::read_to_string(good).unwrap();
        assert!(written.contains("still_works"));
        assert!(!written.contains("payload"));
        drop(logger);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn movement_stream_flushes_first_then_batches_records() {
        let root = test_dir("flush");
        let lane = LaneId(1);
        let path = diagnostic_path(&root, DiagnosticStream::MovementHeartbeat, lane);
        let mut writer = JsonlWriter::open(&path, FlushPolicy::FirstThenEvery(5)).unwrap();
        writer.write("first", json!({})).unwrap();
        for _ in 0..4 {
            writer.write("batched", json!({})).unwrap();
        }
        assert_eq!(std::fs::read_to_string(&path).unwrap().lines().count(), 1);
        writer.write("flush_batch", json!({})).unwrap();
        assert_eq!(std::fs::read_to_string(path).unwrap().lines().count(), 6);
        drop(writer);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn full_queue_drops_without_waiting_and_counts_records() {
        let (sender, _receiver) = mpsc::sync_channel(1);
        let dropped = AtomicU64::new(0);
        let command = || Command::Record {
            stream: DiagnosticStream::Network,
            lane: LaneId(1),
            record_type: "test".into(),
            fields: serde_json::json!({"metadata": true}),
        };
        assert!(sender.try_send(command()).is_ok());
        let started = std::time::Instant::now();
        try_enqueue(&sender, &dropped, command());
        assert!(started.elapsed() < std::time::Duration::from_millis(50));
        assert_eq!(dropped.load(Ordering::Relaxed), 1);
    }

    fn test_dir(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "wow-bot-structured-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        root
    }
}
