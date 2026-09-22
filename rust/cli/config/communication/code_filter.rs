//! Hiding fenced code while a turn is still streaming, without hiding
//! anything else.
//!
//! Split out of `cli/config/communication.rs`, which had grown past the
//! module line cap.

/// Replace every fenced code block in `text` with a one-line placeholder.
pub(crate) fn hide_code_blocks(text: &str) -> String {
    let mut filter = CodeFilter::default();
    let mut out = filter.push(text);
    out.push_str(&filter.finish());
    out
}

/// Streaming fence filter. Text outside ``` / ~~~ fences passes through as
/// soon as its line is complete; fenced lines are swallowed and the closing
/// fence emits `[code hidden: N lines]`. Feed pieces with [`CodeFilter::push`]
/// and drain the tail with [`CodeFilter::finish`].
#[derive(Default)]
pub(crate) struct CodeFilter {
    pending: String,
    fence: Option<Fence>,
    hidden_lines: usize,
}

#[derive(Clone, Copy)]
struct Fence {
    marker: char,
    length: usize,
}

impl CodeFilter {
    pub(crate) fn push(&mut self, piece: &str) -> String {
        self.pending.push_str(piece);
        let mut out = String::new();
        while let Some(newline) = self.pending.find('\n') {
            let line = self.pending.drain(..=newline).collect::<String>();
            self.consume_line(&line, &mut out);
        }
        out
    }

    pub(crate) fn finish(&mut self) -> String {
        let mut out = String::new();
        if !self.pending.is_empty() {
            let line = std::mem::take(&mut self.pending);
            self.consume_line(&line, &mut out);
        }
        if self.fence.take().is_some() {
            out.push_str(&placeholder(self.hidden_lines));
            self.hidden_lines = 0;
        }
        out
    }

    fn consume_line(&mut self, line: &str, out: &mut String) {
        let content = line.trim_start_matches(' ');
        match self.fence {
            Some(fence) => {
                if closes_fence(content, fence) {
                    self.fence = None;
                    out.push_str(&placeholder(self.hidden_lines));
                    if line.ends_with('\n') {
                        out.push('\n');
                    }
                    self.hidden_lines = 0;
                } else {
                    self.hidden_lines += 1;
                }
            }
            None => match opens_fence(content) {
                Some(fence) => {
                    self.fence = Some(fence);
                    self.hidden_lines = 0;
                }
                None => out.push_str(line),
            },
        }
    }
}

fn placeholder(lines: usize) -> String {
    format!(
        "[code hidden: {lines} line{}]",
        if lines == 1 { "" } else { "s" }
    )
}

fn fence_run(content: &str) -> Option<Fence> {
    let marker = content.chars().next().filter(|c| matches!(c, '`' | '~'))?;
    let length = content.chars().take_while(|c| *c == marker).count();
    (length >= 3).then_some(Fence { marker, length })
}

fn opens_fence(content: &str) -> Option<Fence> {
    let fence = fence_run(content)?;
    let info = &content[fence.length..];
    // A backtick fence cannot carry backticks in its info string.
    (fence.marker != '`' || !info.contains('`')).then_some(fence)
}

fn closes_fence(content: &str, open: Fence) -> bool {
    match fence_run(content) {
        Some(run) if run.marker == open.marker && run.length >= open.length => {
            content[run.length..].trim().is_empty()
        }
        _ => false,
    }
}
