#[derive(Debug, Clone)]
pub struct ChatDeltaEvent {
    pub session_id: String,
    pub message_id: String,
    pub delta: String,
    pub chunk_type: String,
    pub is_final: bool,
}

#[derive(Debug, Clone)]
pub struct ChatCompleteEvent {
    pub session_id: String,
    pub message_id: String,
}
