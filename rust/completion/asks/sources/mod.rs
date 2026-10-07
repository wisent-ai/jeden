//! The two ways an ask reaches the operator: a review that blocks a task on
//! something only the operator holds, and an `ask_user` question.

pub(crate) mod ask_user;
pub(crate) mod review;
