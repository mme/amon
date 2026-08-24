//! The managed region amon keeps inside a file that is not amon's.
//!
//! Two files need this: the shell rc that carries the agent aliases, and the
//! Hyprland bindings that carry one `require`. They differ only in how their
//! language spells a comment, so the markers are a parameter and everything
//! else — the splice, the removal, the refusal to touch a damaged fence — is
//! shared. The rules below were paid for once in [`crate::alias`] and are not
//! worth learning twice.
//!
//! The shape is conda's, deliberately: people already read `>>> name >>>` as a
//! machine-managed region whose insides are not theirs to edit.

use std::path::Path;

/// The markers delimiting one managed region.
pub(crate) struct Fence {
    pub(crate) begin: &'static str,
    pub(crate) end: &'static str,
}

/// For files whose comments start with `#` — shell rc.
pub(crate) const HASH: Fence = Fence {
    begin: "# >>> amon >>>",
    end: "# <<< amon <<<",
};

/// For files whose comments start with `--` — Lua.
pub(crate) const DASH: Fence = Fence {
    begin: "-- >>> amon >>>",
    end: "-- <<< amon <<<",
};

impl Fence {
    /// Whether an opening marker is missing its closing one. The walkers below
    /// treat everything inside a fence as amon's to rewrite, so a truncated
    /// fence would swallow the rest of the user's file — every writer checks
    /// this first and refuses to edit rather than destroy.
    pub(crate) fn unterminated(&self, existing: &str) -> bool {
        let mut in_block = false;
        for line in existing.lines() {
            let trimmed = line.trim();
            if !in_block && trimmed == self.begin {
                in_block = true;
            } else if in_block && trimmed == self.end {
                in_block = false;
            }
        }
        in_block
    }

    /// What to say instead of editing a file whose fence is damaged.
    pub(crate) fn truncated_note(&self, path: &Path) -> String {
        format!(
            "the amon block in {} is missing its closing `{}` — repair it by hand before amon edits this file",
            path.display(),
            self.end
        )
    }

    /// The lines outside every fenced block, and where the first block began.
    /// Callers have already refused files where [`Fence::unterminated`] holds.
    fn split<'a>(&self, existing: &'a str) -> (Vec<&'a str>, Option<usize>) {
        let mut kept = Vec::new();
        let mut in_block = false;
        let mut first_block = None;
        for line in existing.lines() {
            let trimmed = line.trim();
            if !in_block && trimmed == self.begin {
                in_block = true;
                first_block.get_or_insert(kept.len());
            } else if in_block {
                if trimmed == self.end {
                    in_block = false;
                }
            } else {
                kept.push(line);
            }
        }
        (kept, first_block)
    }

    /// Replaces the fenced block — every fenced block, merged into one at the
    /// first's position — or appends one when none is there yet. Collapsing
    /// duplicates matters because a hand-maintained file can end up with more
    /// than one; leaving a second behind would double whatever it holds.
    pub(crate) fn with_block(&self, existing: &str, block: &str) -> String {
        let (kept, first_block) = self.split(existing);

        let mut out = String::new();
        let index = first_block.unwrap_or(kept.len());
        for line in &kept[..index] {
            out.push_str(line);
            out.push('\n');
        }
        if first_block.is_none() && !out.is_empty() {
            out.push('\n');
        }
        out.push_str(block);
        out.push('\n');
        for line in &kept[index..] {
            out.push_str(line);
            out.push('\n');
        }
        out
    }

    /// Removes every fenced block, leaving the rest of the file as it was.
    pub(crate) fn without_block(&self, existing: &str) -> Option<String> {
        let (kept, first_block) = self.split(existing);
        first_block?;

        let mut out = String::new();
        for line in kept {
            out.push_str(line);
            out.push('\n');
        }
        // The block is written with a blank line before it; take that back too,
        // so installing and uninstalling repeatedly does not grow the file.
        while out.ends_with("\n\n") {
            out.pop();
        }
        Some(out)
    }

    /// The block itself: markers around a preface and a body.
    pub(crate) fn block(&self, preface: &[&str], body: &[String]) -> String {
        let mut out = String::from(self.begin);
        for line in preface {
            out.push('\n');
            out.push_str(line);
        }
        for line in body {
            out.push('\n');
            out.push_str(line);
        }
        out.push('\n');
        out.push_str(self.end);
        out
    }
}

/// Whether a path is a symlink, which amon will not write through: a dotfiles
/// checkout linked into place is someone's repository, and editing through the
/// link would commit amon's lines into it.
pub(crate) fn is_symlink(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
}
