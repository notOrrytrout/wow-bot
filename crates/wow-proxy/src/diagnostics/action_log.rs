use wow_domain::ActionId;
#[derive(Clone, Debug)]
pub struct ActionLogEntry {
    pub action: ActionId,
    pub result: String,
}
#[derive(Clone, Debug, Default)]
pub struct ActionLogCapture {
    pub active: bool,
    pub markers: Vec<String>,
    pub entries: Vec<ActionLogEntry>,
}
impl ActionLogCapture {
    pub fn start(&mut self) {
        self.active = true
    }
    pub fn stop(&mut self) {
        self.active = false
    }
    pub fn mark(&mut self, label: impl Into<String>) {
        if self.active {
            self.markers.push(label.into())
        }
    }
    pub fn push(&mut self, e: ActionLogEntry) {
        if self.active {
            self.entries.push(e)
        }
    }
}
pub fn redact_payload(kind: &str, body: &[u8]) -> Vec<u8> {
    if matches!(kind, "chat" | "warden" | "auth" | "session_key") {
        b"[redacted]".to_vec()
    } else {
        body.to_vec()
    }
}
