//! Who may write a memory. Enforced for every write (agent tools and Settings alike), so no
//! caller can store a memory that came from document, web or tool content.

use serde::{Deserialize, Serialize};
use shodh_ontology::ExtractorKind;

use super::MemoryError;

/// Provenance source of memories entered in Settings → Memory.
pub const SETTINGS_SOURCE: &str = "settings://memory";

/// Provenance source prefix of memories written during a conversation:
/// `conversation://<conversation id>/turn/<run id>`.
pub const CONVERSATION_SOURCE_PREFIX: &str = "conversation://";

/// The provenance source of a memory written in a conversation turn.
pub fn conversation_source(conversation_id: &str, run_id: &str) -> String {
    format!("{CONVERSATION_SOURCE_PREFIX}{conversation_id}/turn/{run_id}")
}

/// The user's authorisation for one write.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WriteAuthority {
    /// The user entered or edited the memory in the app (Settings → Memory).
    UserInterface,
    /// The user approved this exact write in an approval prompt during a conversation.
    UserApproval {
        /// The approved step.
        step_id: String,
    },
    /// The user turned on automatic learning (Settings → Memory → "Learn from
    /// conversations: automatic") and this LLM-formulated statement from one of the user's
    /// own turns met that policy: high confidence and not sensitive. Only the learning
    /// pipeline writes with this authority, and the memory service re-checks sensitivity
    /// before storing.
    LearnPolicy {
        /// The suggestion the write applies.
        proposal_id: String,
    },
}

impl WriteAuthority {
    /// Stable name for the audit log.
    pub fn label(&self) -> &'static str {
        match self {
            WriteAuthority::UserInterface => "user_interface",
            WriteAuthority::UserApproval { .. } => "user_approval",
            WriteAuthority::LearnPolicy { .. } => "learn_policy",
        }
    }
}

/// Where a memory comes from and who authorised it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Origin {
    /// Provenance source: [`SETTINGS_SOURCE`] or a [`conversation_source`].
    pub source: String,
    /// Who formulated the statement: the user, or an LLM proposing it from the user's turn.
    pub extractor: ExtractorKind,
    /// Version of the extractor (the app version for the user, the model id for an LLM).
    pub extractor_version: String,
    /// Confidence in `[0, 1]` (1 for user-stated facts).
    pub confidence: f64,
    /// The user's authorisation.
    pub authority: WriteAuthority,
}

impl Origin {
    /// A memory the user entered in Settings → Memory.
    pub fn user_interface(app_version: &str) -> Self {
        Self {
            source: SETTINGS_SOURCE.to_string(),
            extractor: ExtractorKind::User,
            extractor_version: app_version.to_string(),
            confidence: 1.0,
            authority: WriteAuthority::UserInterface,
        }
    }

    /// A memory the agent proposed in a conversation turn and the user approved.
    pub fn approved_in_conversation(
        conversation_id: &str,
        run_id: &str,
        step_id: &str,
        app_version: &str,
    ) -> Self {
        Self {
            source: conversation_source(conversation_id, run_id),
            extractor: ExtractorKind::User,
            extractor_version: app_version.to_string(),
            confidence: 1.0,
            authority: WriteAuthority::UserApproval {
                step_id: step_id.to_string(),
            },
        }
    }
}

/// Checks that a write is authorised. The rules:
/// - rule and GLiNER extraction work on documents: never a memory;
/// - the source must be the user's own surface: Settings or a conversation turn — never a
///   document path, a URL or any other source;
/// - Settings writes are the user's own (`UserInterface`, extractor `User`);
/// - conversation writes need the user's approval of that exact write (an LLM-formulated
///   statement included, which also needs a confidence);
/// - automatic learning writes only LLM-formulated statements from a conversation turn.
pub fn check_write_origin(origin: &Origin) -> Result<(), MemoryError> {
    let forbidden = |reason: &str| Err(MemoryError::Forbidden(reason.to_string()));
    if matches!(
        origin.extractor,
        ExtractorKind::Rule | ExtractorKind::Gliner
    ) {
        return forbidden(
            "memories cannot come from document extraction; only the user can create them",
        );
    }
    if !origin.confidence.is_finite() || !(0.0..=1.0).contains(&origin.confidence) {
        return forbidden("confidence must be within [0, 1]");
    }
    let from_settings = origin.source == SETTINGS_SOURCE;
    let from_conversation = origin
        .source
        .strip_prefix(CONVERSATION_SOURCE_PREFIX)
        .is_some_and(|rest| {
            let mut parts = rest.split("/turn/");
            matches!(
                (parts.next(), parts.next(), parts.next()),
                (Some(conversation), Some(run), None)
                    if !conversation.is_empty() && !run.is_empty() && !run.contains('/')
            )
        });
    match (&origin.authority, origin.extractor) {
        (WriteAuthority::UserInterface, ExtractorKind::User) if from_settings => Ok(()),
        (WriteAuthority::UserInterface, _) => {
            forbidden("only memories the user enters in Settings count as entered by the user")
        }
        (WriteAuthority::UserApproval { step_id }, ExtractorKind::User | ExtractorKind::Llm)
            if from_conversation && !step_id.trim().is_empty() =>
        {
            Ok(())
        }
        (WriteAuthority::UserApproval { .. }, _) => forbidden(
            "a memory proposed in a conversation must come from that conversation and be approved by the user",
        ),
        (WriteAuthority::LearnPolicy { proposal_id }, ExtractorKind::Llm)
            if from_conversation && !proposal_id.trim().is_empty() =>
        {
            Ok(())
        }
        (WriteAuthority::LearnPolicy { .. }, _) => forbidden(
            "automatic learning only stores statements an LLM formulated from the user's own conversation turn",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn origin(source: &str, extractor: ExtractorKind, authority: WriteAuthority) -> Origin {
        Origin {
            source: source.to_string(),
            extractor,
            extractor_version: "test".to_string(),
            confidence: 0.9,
            authority,
        }
    }

    fn approval() -> WriteAuthority {
        WriteAuthority::UserApproval {
            step_id: "step-1".to_string(),
        }
    }

    #[test]
    fn user_surfaces_are_allowed() {
        assert!(check_write_origin(&Origin::user_interface("1.0")).is_ok());
        assert!(
            check_write_origin(&Origin::approved_in_conversation("c1", "r1", "s1", "1.0")).is_ok()
        );
        // An LLM-formulated statement from the user's turn, approved by the user.
        let llm = origin("conversation://c1/turn/r1", ExtractorKind::Llm, approval());
        assert!(check_write_origin(&llm).is_ok());
    }

    #[test]
    fn document_web_and_tool_content_is_rejected() {
        let cases = [
            // Extracted from a document by rules or GLiNER.
            origin("C:/docs/contract.pdf", ExtractorKind::Rule, approval()),
            origin(
                "conversation://c1/turn/r1",
                ExtractorKind::Gliner,
                approval(),
            ),
            // A document or web page as the source, even with an approval.
            origin("C:/docs/contract.pdf", ExtractorKind::User, approval()),
            origin("https://example.com/page", ExtractorKind::Llm, approval()),
            origin("note://n1", ExtractorKind::User, approval()),
            // A malformed conversation source.
            origin("conversation://c1", ExtractorKind::User, approval()),
            origin(
                "conversation://c1/turn/r1/extra",
                ExtractorKind::User,
                approval(),
            ),
            // No approval for a conversation write.
            origin(
                "conversation://c1/turn/r1",
                ExtractorKind::User,
                WriteAuthority::UserApproval {
                    step_id: " ".into(),
                },
            ),
            // "Entered in Settings" claimed for something that was not.
            origin(
                "conversation://c1/turn/r1",
                ExtractorKind::User,
                WriteAuthority::UserInterface,
            ),
            origin(
                SETTINGS_SOURCE,
                ExtractorKind::Llm,
                WriteAuthority::UserInterface,
            ),
        ];
        for case in cases {
            assert!(
                matches!(check_write_origin(&case), Err(MemoryError::Forbidden(_))),
                "{case:?}"
            );
        }
        // Automatic learning: only LLM statements from a conversation turn.
        let learned = |source: &str, extractor| {
            origin(
                source,
                extractor,
                WriteAuthority::LearnPolicy {
                    proposal_id: "p1".into(),
                },
            )
        };
        assert!(
            check_write_origin(&learned("conversation://c1/turn/r1", ExtractorKind::Llm)).is_ok()
        );
        for case in [
            learned("conversation://c1/turn/r1", ExtractorKind::User),
            learned(SETTINGS_SOURCE, ExtractorKind::Llm),
            learned("https://example.com", ExtractorKind::Llm),
            learned("conversation://c1/turn/r1", ExtractorKind::Rule),
        ] {
            assert!(check_write_origin(&case).is_err(), "{case:?}");
        }
        let mut bad_confidence = Origin::user_interface("1.0");
        bad_confidence.confidence = f64::NAN;
        assert!(check_write_origin(&bad_confidence).is_err());
    }
}
