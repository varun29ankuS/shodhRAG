//! Figures and equations of library papers, read from the PDF on demand and cached.
//!
//! Parsing a paper's layout takes a moment, and the agent, the figure blocks of an answer
//! and the paper page all ask for the same papers, so the last few results are kept
//! (keyed by the file's path, size and modification time, and the `.tex` files that may
//! be its source). Nothing is stored: the objects are derived from the file every time
//! it changes.

use std::collections::VecDeque;
use std::path::Path;
use std::sync::{Arc, Mutex};

use serde::Serialize;

use super::equations::{equations_of, tex_equations, Equation, EquationOrigin, MAX_TEX_BYTES};
use super::figures::{figures_of, Figure};
use super::{blocking, file_name, path_key, ResearchError, ResearchResult};
use crate::processing::document_model::{PageInfo, StructuredDocument};

/// Papers kept in the cache.
const CACHE_PAPERS: usize = 6;
/// Characters of `\input` files inlined into one source.
const MAX_INLINED_BYTES: usize = MAX_TEX_BYTES;

/// The figures and equations of one paper.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PaperParts {
    pub file_path: String,
    pub file_name: String,
    pub pages: Vec<PageInfo>,
    pub figures: Vec<Figure>,
    pub equations: Vec<Equation>,
    /// The `.tex` file equations were matched to, when one matched.
    pub tex_source: Option<String>,
}

impl PaperParts {
    /// Builds the parts of a parsed paper, matching equations against the candidate
    /// sources (path and text); the candidate with the most matched equations wins.
    pub fn build(file_path: &str, doc: &StructuredDocument, sources: &[(String, String)]) -> Self {
        let mut equations = equations_of(doc, None);
        let mut tex_source = None;
        let mut best = 0usize;
        for (path, text) in sources {
            let tex = tex_equations(text);
            if tex.is_empty() {
                continue;
            }
            let candidate = equations_of(doc, Some((path, &tex)));
            let matched = candidate
                .iter()
                .filter(|e| e.origin == EquationOrigin::Source)
                .count();
            if matched > best {
                best = matched;
                equations = candidate;
                tex_source = Some(path.clone());
            }
        }
        PaperParts {
            file_path: file_path.to_string(),
            file_name: file_name(file_path),
            pages: doc.pages.clone(),
            figures: figures_of(doc),
            equations,
            tex_source,
        }
    }
}

/// `.tex` files among `indexed` that may hold the source of the PDF at `pdf_path`: those
/// in its folder or a folder directly inside it, and any with the PDF's file stem.
pub fn tex_candidates(pdf_path: &str, indexed: &[String]) -> Vec<String> {
    let pdf = path_key(pdf_path);
    let (dir, stem) = match pdf.rsplit_once('/') {
        Some((d, f)) => (d.to_string(), f.trim_end_matches(".pdf").to_string()),
        None => (String::new(), pdf.trim_end_matches(".pdf").to_string()),
    };
    let mut out: Vec<String> = indexed
        .iter()
        .filter(|s| !s.contains("://"))
        .filter(|s| s.to_ascii_lowercase().ends_with(".tex"))
        .filter(|s| {
            let key = path_key(s);
            let (tex_dir, tex_file) = key.rsplit_once('/').unwrap_or(("", key.as_str()));
            let parent = tex_dir.rsplit_once('/').map_or("", |(p, _)| p);
            tex_dir == dir || parent == dir || tex_file.trim_end_matches(".tex") == stem
        })
        .cloned()
        .collect();
    out.sort();
    out.dedup();
    out
}

fn read_tex(path: &str) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() > MAX_TEX_BYTES as u64 {
        return None;
    }
    std::fs::read_to_string(path).ok()
}

/// The candidate sources as whole documents: each file with `\begin{document}` with the
/// `\input`/`\include` files it names inlined (only files among the candidates), or every
/// candidate on its own when none is a main file.
pub fn assemble_sources(candidates: &[String]) -> Vec<(String, String)> {
    let texts: Vec<(String, String)> = candidates
        .iter()
        .filter_map(|p| read_tex(p).map(|t| (p.clone(), t)))
        .collect();
    let mains: Vec<&(String, String)> = texts
        .iter()
        .filter(|(_, t)| t.contains("\\begin{document}"))
        .collect();
    if mains.is_empty() {
        return texts;
    }
    let keyed: Vec<(String, &str)> = texts
        .iter()
        .map(|(p, t)| (path_key(p), t.as_str()))
        .collect();
    mains
        .into_iter()
        .map(|(path, text)| (path.clone(), inline_inputs(path, text, &keyed)))
        .collect()
}

fn inline_inputs(main: &str, text: &str, files: &[(String, &str)]) -> String {
    let dir = Path::new(main).parent().map(Path::to_path_buf);
    let mut out = String::with_capacity(text.len());
    let mut inlined = 0usize;
    let mut rest = text;
    while let Some(at) = ["\\input{", "\\include{"]
        .iter()
        .filter_map(|cmd| rest.find(cmd).map(|i| (i, cmd.len())))
        .min_by_key(|(i, _)| *i)
    {
        let (start, len) = at;
        out.push_str(&rest[..start]);
        let after = &rest[start + len..];
        let Some(close) = after.find('}') else {
            out.push_str(&rest[start..]);
            return out;
        };
        let name = after[..close].trim();
        let file = if name.ends_with(".tex") {
            name.to_string()
        } else {
            format!("{name}.tex")
        };
        let resolved = dir
            .as_ref()
            .map(|d| d.join(&file))
            .map(|p| path_key(&p.to_string_lossy()));
        let body = resolved.and_then(|key| files.iter().find(|(k, _)| *k == key).map(|(_, t)| *t));
        match body {
            Some(body) if inlined + body.len() <= MAX_INLINED_BYTES => {
                inlined += body.len();
                out.push('\n');
                out.push_str(body);
                out.push('\n');
            }
            _ => out.push_str(&rest[start..start + len + close + 1]),
        }
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    out
}

struct Entry {
    key: String,
    parts: Arc<PaperParts>,
}

/// The cache of parsed papers. Shared by the commands and the agent tools.
#[derive(Default)]
pub struct PaperObjects {
    cache: Mutex<VecDeque<Entry>>,
}

impl PaperObjects {
    pub fn new() -> Self {
        Self::default()
    }

    /// The figures and equations of the PDF at `pdf_path` (an indexed file the caller has
    /// checked), with `tex_candidates` as possible LaTeX sources.
    pub async fn parts(
        &self,
        pdf_path: &str,
        tex_candidates: Vec<String>,
    ) -> ResearchResult<Arc<PaperParts>> {
        let path = pdf_path.to_string();
        let meta_path = path.clone();
        let meta = blocking(move || {
            std::fs::metadata(&meta_path).map_err(|e| {
                ResearchError::Pdf(format!("{} could not be read: {e}", file_name(&meta_path)))
            })
        })
        .await?;
        let modified = meta
            .modified()
            .ok()
            .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_millis());
        let key = format!(
            "{}|{}|{}|{}",
            path_key(&path),
            meta.len(),
            modified,
            tex_candidates.join(";")
        );
        if let Some(hit) = self.lookup(&key) {
            return Ok(hit);
        }
        let parts = blocking(move || {
            let bytes = std::fs::read(&path).map_err(|e| {
                ResearchError::Pdf(format!("{} could not be read: {e}", file_name(&path)))
            })?;
            let doc = crate::processing::pdf_layout::parse_pdf_layout(&bytes)
                .map_err(|e| ResearchError::Pdf(format!("The PDF could not be parsed: {e}")))?;
            let sources = assemble_sources(&tex_candidates);
            Ok(PaperParts::build(&path, &doc, &sources))
        })
        .await?;
        let parts = Arc::new(parts);
        self.store(key, parts.clone());
        Ok(parts)
    }

    fn lookup(&self, key: &str) -> Option<Arc<PaperParts>> {
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        let at = cache.iter().position(|e| e.key == key)?;
        let entry = cache.remove(at)?;
        let parts = entry.parts.clone();
        cache.push_back(entry);
        Some(parts)
    }

    fn store(&self, key: String, parts: Arc<PaperParts>) {
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        cache.retain(|e| e.key != key);
        cache.push_back(Entry { key, parts });
        while cache.len() > CACHE_PAPERS {
            cache.pop_front();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::processing::document_model::{BBox, Block, BlockKind};

    #[test]
    fn tex_candidates_are_the_papers_folder_its_subfolders_and_its_stem() {
        let indexed = vec![
            "c:/papers/delta.pdf".to_string(),
            "c:/papers/delta.tex".to_string(),
            "c:/papers/src/main.tex".to_string(),
            "c:/papers/src/deep/sec.tex".to_string(),
            "c:/elsewhere/delta.tex".to_string(),
            "c:/elsewhere/other.tex".to_string(),
            "https://example.org/x.tex".to_string(),
        ];
        let found = tex_candidates("c:/papers/delta.pdf", &indexed);
        assert_eq!(
            found,
            [
                "c:/elsewhere/delta.tex",
                "c:/papers/delta.tex",
                "c:/papers/src/main.tex"
            ]
        );
    }

    #[test]
    fn main_files_get_their_inputs_inlined() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("main.tex");
        let sec = dir.path().join("method.tex");
        std::fs::write(
            &main,
            "\\begin{document}\n\\input{method}\n\\include{missing}\n\\end{document}",
        )
        .unwrap();
        std::fs::write(&sec, "\\begin{equation}a = b\\end{equation}").unwrap();
        let candidates = vec![
            main.to_string_lossy().to_string(),
            sec.to_string_lossy().to_string(),
        ];
        let sources = assemble_sources(&candidates);
        assert_eq!(sources.len(), 1);
        assert!(sources[0].1.contains("a = b"), "{}", sources[0].1);
        // A file that is not a candidate is left as written.
        assert!(sources[0].1.contains("\\include{missing}"));
        assert_eq!(tex_equations(&sources[0].1)[0].number.as_deref(), Some("1"));
    }

    #[test]
    fn the_best_matching_source_wins() {
        let doc = StructuredDocument {
            pages: Vec::new(),
            blocks: vec![
                Block::new(BlockKind::Equation, "ot = Stqt (1)")
                    .on_page(2, Some(BBox::new(100.0, 300.0, 400.0, 320.0))),
                Block::new(
                    BlockKind::Figure {
                        caption: "Figure 1: Model.".into(),
                    },
                    "Figure 1: Model.",
                )
                .on_page(2, Some(BBox::new(100.0, 500.0, 400.0, 512.0))),
            ],
        };
        let sources = vec![
            (
                "c:/p/other.tex".to_string(),
                "\\begin{document}\\begin{equation}L = \\sum_i y_i\\end{equation}\\end{document}"
                    .to_string(),
            ),
            (
                "c:/p/main.tex".to_string(),
                "\\begin{document}\\begin{equation}o_t = S_t q_t\\end{equation}\\end{document}"
                    .to_string(),
            ),
        ];
        let parts = PaperParts::build("c:/p/paper.pdf", &doc, &sources);
        assert_eq!(parts.tex_source.as_deref(), Some("c:/p/main.tex"));
        assert_eq!(parts.equations[0].latex, "o_t = S_t q_t");
        assert_eq!(parts.figures.len(), 1);
        assert_eq!(parts.file_name, "paper.pdf");
    }

    #[test]
    fn the_cache_keeps_the_most_recent_papers() {
        let objects = PaperObjects::new();
        let parts = Arc::new(PaperParts::build(
            "a.pdf",
            &StructuredDocument::default(),
            &[],
        ));
        for i in 0..(CACHE_PAPERS + 2) {
            objects.store(format!("k{i}"), parts.clone());
        }
        assert!(objects.lookup("k0").is_none());
        assert!(objects.lookup(&format!("k{}", CACHE_PAPERS + 1)).is_some());
    }
}
