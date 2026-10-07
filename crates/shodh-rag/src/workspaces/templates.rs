//! Templates a workspace can start from: an icon, a colour and starting instructions.
//! Templates only fill the instructions; they never promise behaviour the app does not
//! have (the paper-writing template asks for BibTeX entries built from source metadata,
//! which the model writes as text; there is no reference manager behind it).

use serde::Serialize;

/// A starting point for a workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceTemplate {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub icon: &'static str,
    pub color: &'static str,
    pub instructions: &'static str,
}

/// Id of the template without instructions.
pub const BLANK: &str = "blank";

/// Every template, blank first.
pub const TEMPLATES: &[WorkspaceTemplate] = &[
    WorkspaceTemplate {
        id: BLANK,
        name: "Blank",
        description: "No instructions; add sources and write your own.",
        icon: "folder",
        color: "slate",
        instructions: "",
    },
    WorkspaceTemplate {
        id: "literature_review",
        name: "Literature review",
        description: "Survey papers on a question: methods, findings, disagreements and gaps.",
        icon: "book-open",
        color: "blue",
        instructions: "This workspace is a literature review.\n\
- Answer from the papers in this workspace and cite every claim to a passage.\n\
- When comparing papers, give each paper's method, data, main result and stated limitations; use a table when more than two papers are compared.\n\
- Say explicitly where papers disagree, and where the sources do not answer the question (a gap) rather than filling it from general knowledge.\n\
- Prefer the authors' own reported numbers; give units and the setting they were measured in.\n\
- Name papers by first author and year (e.g. Smith 2021).",
    },
    WorkspaceTemplate {
        id: "grant_proposal",
        name: "Grant proposal",
        description: "Draft aims, significance and approach from your prior work and the call.",
        icon: "landmark",
        color: "amber",
        instructions: "This workspace is for writing a grant proposal.\n\
- Treat the funding call in the sources as the requirements: check every draft against its sections, page limits and review criteria, and point out anything missing.\n\
- Ground preliminary results and claims of prior work in the sources, with citations; mark anything not supported by a source as needing evidence.\n\
- Write specific, measurable aims; for each aim give the hypothesis, the approach and the expected outcome.\n\
- Keep a formal, concise tone and avoid unexplained jargon.",
    },
    WorkspaceTemplate {
        id: "paper_writing",
        name: "Paper writing",
        description: "Write and revise a paper with citations to the papers you collected.",
        icon: "pen-line",
        color: "violet",
        instructions: "This workspace is for writing a research paper.\n\
- Cite related work only from the papers in this workspace, and check each citation against the passage it relies on.\n\
- When I ask for references, write them as BibTeX entries built only from the metadata shown in the sources (authors, title, venue, year, DOI or arXiv id); leave out any field the sources do not show instead of guessing it, and use keys like smith2021keyword.\n\
- Keep my terminology and notation consistent with the draft; point out where they drift.\n\
- When revising, show the changed text and say briefly why it changed.",
    },
    WorkspaceTemplate {
        id: "client_audit",
        name: "Client audit",
        description: "Review a client's documents against requirements and record findings.",
        icon: "clipboard-check",
        color: "teal",
        instructions: "This workspace is an audit of one client's documents.\n\
- Only use this workspace's documents; never bring in other clients' material.\n\
- For each finding give: the requirement, what the documents show (cited, with page), the gap, and its severity (high, medium or low).\n\
- Quote figures, dates and clause numbers exactly as written in the source.\n\
- Say clearly when the documents do not contain the evidence needed to decide, instead of assuming compliance.",
    },
];

/// The template with `id`, if any.
pub fn template(id: &str) -> Option<&'static WorkspaceTemplate> {
    TEMPLATES.iter().find(|t| t.id == id.trim())
}
