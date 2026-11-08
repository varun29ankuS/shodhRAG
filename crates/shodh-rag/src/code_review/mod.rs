//! Code review: the shared diff, the fresh-eyes reviewer and the structural
//! view an Architecture review reads.
//!
//! The reviewer is one stateless model call ([`run_review`]): no tools, no
//! session, no memories, no conversation. Its prompt is a pure function of a
//! [`ReviewRequest`] (the code or diff lines selected, their paths and
//! language, the check, the user's own notes) and, for an Architecture
//! review, the [`StructureView`] Shodh built from the folder. The goldfish
//! rule is structural: there is no argument through which anything else
//! could reach it, and the request type refuses unknown fields.
//!
//! Running a check the reviewer proposes is not done here: the user sends it
//! to the Code conversation, where the conversation's mode and Code mode's
//! command rules decide it like any other command.

pub mod diff;
pub mod report;
pub mod request;
pub mod secrets;
pub mod structure;

#[cfg(test)]
mod tests;

pub use report::{
    visible_text, Citation, Expect, Finding, Replacement, ReviewReport, Severity, Side, VerifyStep,
};
pub use request::{
    check_request, reviewer_prompt, LineKind, PieceLine, ReviewCheck, ReviewPiece, ReviewRequest,
};
pub use structure::{StructureSource, StructureView};

use crate::research::results::TextModel;

/// Most output tokens a review reply may use.
pub const MAX_REPLY_TOKENS: usize = 6_000;

/// Review `request` with `model`, in one call that sees only the request (and
/// `structure` for an Architecture review). The reply is checked: citations
/// outside what was shown are dropped, findings ranked worst first.
pub async fn run_review(
    model: &dyn TextModel,
    request: &ReviewRequest,
    structure: Option<&StructureView>,
) -> Result<ReviewReport, String> {
    check_request(request)?;
    if request.check == ReviewCheck::Architecture
        && request.pieces.is_empty()
        && structure.is_none()
    {
        return Err("An architecture review needs a folder of the project.".to_string());
    }
    let marker = uuid::Uuid::new_v4().simple().to_string();
    let prompt = reviewer_prompt(request, structure, &marker);
    let reply = model.complete_reply(&prompt, MAX_REPLY_TOKENS).await?;
    match report::parse_reply(request, structure, &reply.text) {
        Some(report) => Ok(report),
        None if reply.truncated => {
            Err("The review was cut off before it finished. Review a smaller part.".to_string())
        }
        None => Err("The reviewer's answer could not be read. Try again.".to_string()),
    }
}
