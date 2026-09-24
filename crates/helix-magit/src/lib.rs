//! The engine behind Helix's Magit-style Git client.
//!
//! This crate holds everything that does not need the editor: a structural
//! model of a diff, the transient-menu model, and a thin Git layer over
//! `gix`. Rendering and key handling live in `helix-term`.

pub mod command;
pub mod diff;
pub mod patch;
pub mod repository;
pub mod status;
pub mod transient;

pub use command::{resolve, GitCommand, GitOutput, Plan, Requirement};
pub use diff::{
    parse_unified_diff, render_patch, DiffHunk, DiffLine, DiffLineKind, FileDiff, FileStatus,
    HunkHeader,
};
pub use patch::{apply_patch, build_partial_patch, build_reverse_patch, ApplyError, Selection};
pub use repository::{Repository, StatusEntry, Unmerged};
pub use transient::{
    MagitCommand, TransientAction, TransientArgument, TransientGroup, TransientMenu,
    TransientOption, TransientSwitch,
};
