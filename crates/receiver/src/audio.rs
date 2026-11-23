//! Phone microphone playback (filled in by phase 8).

pub struct Sink;

impl Sink {
    pub fn new(_device: &str) -> Sink {
        Sink
    }
    pub fn push(&mut self, _pcm: &[u8]) {}
}

pub fn render_devices() -> Vec<String> {
    Vec::new()
}
