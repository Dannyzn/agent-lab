use agent_protocol::{BoxError, EventSink};
use serde_json::Value;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};

mod session;
pub use session::{
    FinalMessage, PendingTool, Record, SessionJournal, SessionState, TurnState, TurnStatus,
};

pub struct JsonlEventSink {
    file: Mutex<File>,
}

impl JsonlEventSink {
    pub fn open(path: &Path) -> io::Result<Self> {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        Ok(Self {
            file: Mutex::new(file),
        })
    }
}

impl EventSink for JsonlEventSink {
    fn emit(&self, event: &Value) -> Result<(), BoxError> {
        let mut file = self
            .file
            .lock()
            .map_err(|_| io::Error::other("event file lock is poisoned"))?;
        serde_json::to_writer(&mut *file, event)?;
        file.write_all(b"\n")?;
        file.flush()?;
        Ok(())
    }
}

#[derive(Default)]
pub struct MemoryEventSink {
    events: Mutex<Vec<Value>>,
}

impl MemoryEventSink {
    pub fn events(&self) -> Vec<Value> {
        self.events.lock().expect("event lock").clone()
    }
}

impl EventSink for MemoryEventSink {
    fn emit(&self, event: &Value) -> Result<(), BoxError> {
        self.events
            .lock()
            .map_err(|_| io::Error::other("event memory lock is poisoned"))?
            .push(event.clone());
        Ok(())
    }
}

pub struct StdoutEventSink;

impl EventSink for StdoutEventSink {
    fn emit(&self, event: &Value) -> Result<(), BoxError> {
        println!("{}", serde_json::to_string(event)?);
        Ok(())
    }
}

pub struct NullEventSink;

impl EventSink for NullEventSink {
    fn emit(&self, _event: &Value) -> Result<(), BoxError> {
        Ok(())
    }
}

pub struct TeeEventSink {
    sinks: Vec<Arc<dyn EventSink>>,
}

impl TeeEventSink {
    pub fn new(sinks: Vec<Arc<dyn EventSink>>) -> Self {
        Self { sinks }
    }
}

impl EventSink for TeeEventSink {
    fn emit(&self, event: &Value) -> Result<(), BoxError> {
        for sink in &self.sinks {
            sink.emit(event)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    #[test]
    fn opens_filename_without_parent_directory() {
        let original = env::current_dir().unwrap();
        let temporary = tempfile::tempdir().unwrap();
        env::set_current_dir(temporary.path()).unwrap();
        let sink = JsonlEventSink::open(Path::new("events.jsonl")).unwrap();
        sink.emit(&serde_json::json!({"type":"test"})).unwrap();
        env::set_current_dir(original).unwrap();
        assert!(temporary.path().join("events.jsonl").exists());
    }
}
