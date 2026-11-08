//! Background file-operation queue.
//!
//! One worker thread drains a job channel and executes Copy / Move / Delete /
//! Rename / Create jobs sequentially. Progress is reported through
//! [`OpEvent`]s; conflicts and recoverable errors suspend the worker until the
//! UI answers via the reply channel.
//!
//! Safety rules (from the roadmap):
//! - move inside one volume → `rename`; across volumes → copy, verify size,
//!   delete source — never delete before the copy succeeds;
//! - deleting anything is only enqueued after explicit UI confirmation;
//! - a failed file aborts neither the queue nor silently: the worker asks
//!   Retry / Skip / Cancel.

mod types;
mod worker;

pub use types::{
    ConflictAction, ErrorAction, OpEvent, OpJob, OpKind, OpReply, OpStage, OpsPlatform,
};
pub use worker::spawn;
