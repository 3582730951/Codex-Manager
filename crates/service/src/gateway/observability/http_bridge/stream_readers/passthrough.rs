use super::*;

pub(crate) struct PassthroughSseUsageReader {
    upstream: BufReader<reqwest::blocking::Response>,
    pending_frame_lines: Vec<String>,
    out_cursor: Cursor<Vec<u8>>,
    usage_collector: Arc<Mutex<PassthroughSseCollector>>,
    finished: bool,
}

impl PassthroughSseUsageReader {
    pub(crate) fn new(
        upstream: reqwest::blocking::Response,
        usage_collector: Arc<Mutex<PassthroughSseCollector>>,
    ) -> Self {
        Self {
            upstream: BufReader::new(upstream),
            pending_frame_lines: Vec::new(),
            out_cursor: Cursor::new(Vec::new()),
            usage_collector,
            finished: false,
        }
    }

    fn update_usage_from_frame(&self, lines: &[String]) {
        let inspection = inspect_sse_frame(lines);
        if inspection.usage.is_none() && inspection.terminal.is_none() {
            return;
        }
        if let Ok(mut collector) = self.usage_collector.lock() {
            if let Some(parsed) = inspection.usage {
                merge_usage(&mut collector.usage, parsed);
            }
            if let Some(terminal) = inspection.terminal {
                collector.saw_terminal = true;
                if let SseTerminal::Err(message) = terminal {
                    collector.terminal_error = Some(message);
                }
            }
        }
    }

    fn next_chunk(&mut self) -> std::io::Result<Vec<u8>> {
        let mut line = String::new();
        let read = self.upstream.read_line(&mut line)?;
        if read == 0 {
            if !self.pending_frame_lines.is_empty() {
                let frame = std::mem::take(&mut self.pending_frame_lines);
                self.update_usage_from_frame(&frame);
            }
            if let Ok(mut collector) = self.usage_collector.lock() {
                if !collector.saw_terminal {
                    collector
                        .terminal_error
                        .get_or_insert_with(|| "stream disconnected before completion".to_string());
                }
            }
            self.finished = true;
            return Ok(Vec::new());
        }
        if line == "\n" || line == "\r\n" {
            if !self.pending_frame_lines.is_empty() {
                let frame = std::mem::take(&mut self.pending_frame_lines);
                self.update_usage_from_frame(&frame);
            }
        } else {
            self.pending_frame_lines.push(line.clone());
        }
        Ok(line.into_bytes())
    }
}

impl Read for PassthroughSseUsageReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        loop {
            let read = self.out_cursor.read(buf)?;
            if read > 0 {
                return Ok(read);
            }
            if self.finished {
                return Ok(0);
            }
            self.out_cursor = Cursor::new(self.next_chunk()?);
        }
    }
}
