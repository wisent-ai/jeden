//! Building the tree's nodes: what each node is made of, and many
//! compactions at once. A message's line and a pair's merge are asked of the
//! cheap model side by side, as many at a time as `Fitting::calls`; a parent
//! waits for the round that builds its halves. Each node is kept as soon as
//! its round ends, in the order it was asked for.

use super::{compress, halves, message_level, name, Entry, Node, Summarize, Tree};

/// How a view's lines are built and merged: the operator's numbers, how
/// many compactions run at once, and the cheap model.
pub struct Fitting<'a> {
    pub summary_bytes: usize,
    pub attempts: usize,
    /// How many compactions run at once.
    pub calls: usize,
    pub high: usize,
    pub low: usize,
    pub summarize: &'a Summarize,
}

/// What a wanted node stands for: one message, or the pair of lines of the
/// level below that node (l, i) merges.
#[derive(Clone, Copy)]
pub enum Of<'a> {
    Message(&'a Entry),
    Pair(u32, u64),
}

/// A node to build and the lines its compaction is shown before its task.
pub struct Wanted<'a> {
    pub of: Of<'a>,
    pub context: String,
}

impl Wanted<'_> {
    fn at(&self) -> (u32, u64) {
        match self.of {
            Of::Message(entry) => (message_level(), entry.i),
            Of::Pair(l, i) => (l, i),
        }
    }
}

/// What a node is made of: its own words when they fit one line, else the
/// compaction the cheap model is asked for, and what a failed one says.
pub(super) enum Made {
    Words(String),
    Asked { task: String, failed: String },
}

impl Made {
    fn line(&self, fitting: &Fitting) -> Result<String, String> {
        match self {
            Self::Words(text) => Ok(text.clone()),
            Self::Asked { task, failed } => compress(
                task,
                fitting.summary_bytes,
                fitting.attempts,
                fitting.summarize,
            )
            .map_err(|error| format!("{failed}: {error}")),
        }
    }
}

impl Tree {
    /// Build every node `wanted` names that the tree does not hold yet, as
    /// many compactions at once as `fitting.calls`, each given its own
    /// context; nodes are kept in the order they are named. A parent's
    /// halves are built first, by an earlier call or earlier in `wanted`.
    pub fn build(&mut self, wanted: &[Wanted], fitting: &Fitting) -> Result<(), String> {
        let mut pending: Vec<&Wanted> = Vec::new();
        for want in wanted {
            let at = want.at();
            let named = pending.iter().any(|kept| kept.at() == at);
            if self.get(at.0, at.1).is_none() && !named {
                pending.push(want);
            }
        }
        // A round's parents may need the nodes the round before built.
        while let Some(next) = pending.first() {
            // The nodes at the front whose halves are built run this round;
            // a parent waits for the round that builds its halves.
            let ready = pending.iter().take_while(|want| self.ready(want)).count();
            let (l, i) = next.at();
            let round: Vec<&Wanted> = pending.drain(..ready.min(fitting.calls)).collect();
            if round.is_empty() {
                return Err(format!(
                    "node {} cannot be merged before both its halves are built",
                    name(l, i)
                ));
            }
            let made = round
                .iter()
                .map(|want| match want.of {
                    Of::Message(entry) => {
                        Self::leaf_made(entry, &want.context, fitting.summary_bytes)
                    }
                    Of::Pair(l, i) => self.parent_made(l, i, &want.context, fitting.summary_bytes),
                })
                .collect::<Result<Vec<_>, String>>()?;
            let lines = std::thread::scope(|scope| {
                let running: Vec<_> = made
                    .iter()
                    .map(|made| scope.spawn(move || made.line(fitting)))
                    .collect();
                running
                    .into_iter()
                    .map(|running| {
                        running.join().map_err(|_| {
                            "a compaction call panicked, so its line was not built".to_string()
                        })?
                    })
                    .collect::<Result<Vec<String>, String>>()
            })?;
            for (want, text) in round.iter().zip(lines) {
                let (l, i) = want.at();
                self.keep(Node {
                    l,
                    i,
                    size: text.len(),
                    text,
                })?;
            }
        }
        Ok(())
    }

    /// Node (l, i) built from its halves when the tree has none, with
    /// `context` (the lines up to its last message).
    pub fn parent(
        &mut self,
        l: u32,
        i: u64,
        context: String,
        fitting: &Fitting,
    ) -> Result<Node, String> {
        self.build(
            &[Wanted {
                of: Of::Pair(l, i),
                context,
            }],
            fitting,
        )?;
        self.get(l, i).cloned().ok_or_else(|| {
            format!(
                "node {} was built but the tree does not hold it",
                name(l, i)
            )
        })
    }

    /// Whether `want` can be built now: a message's line always, a parent's
    /// once both its halves are.
    fn ready(&self, want: &Wanted) -> bool {
        match want.of {
            Of::Message(_) => true,
            Of::Pair(l, i) => halves(l, i).is_some_and(|((ll, li), (rl, ri))| {
                self.get(ll, li).is_some() && self.get(rl, ri).is_some()
            }),
        }
    }
}
