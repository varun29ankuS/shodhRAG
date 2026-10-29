//! The generated part of the paper corpus ([`crate::papers`]): twelve method
//! papers in six topic families, a long thesis and a long survey of KV cache
//! compression, so that the library is several times larger than the
//! candidate pool a search reranks (the condition under which one long paper
//! can crowd the others out of the results).
//!
//! Each paper has a few hand-written anchor sentences (what the questions
//! ask about) and body text drawn from per-family vocabularies and sentence
//! templates with a seeded generator: the same build always produces the
//! same text. Every method, author, benchmark and number is invented.

use crate::papers::{eq, h, p, refs, Block, PdfPaper};
use crate::papers_text::{Entry, EVICTION_FAMILY, LINEAR_FAMILY, MEMORY_FAMILY, QUANT_FAMILY};

/// A deterministic generator (FNV-1a seed, SplitMix64 steps).
pub(crate) struct Rng(u64);

impl Rng {
    pub(crate) fn new(seed: &str) -> Self {
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for byte in seed.bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
        Self(hash)
    }

    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// A number in `0..n` (`n` > 0).
    pub(crate) fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

/// A topic: its vocabulary, benchmarks and baselines.
pub(crate) struct Family {
    pub name: &'static str,
    pub terms: &'static [&'static str],
    pub benches: &'static [&'static str],
    pub baselines: &'static [&'static str],
}

pub(crate) const LINEAR: Family = Family {
    name: "linear attention",
    terms: &[
        "feature map",
        "recurrent state",
        "decay gate",
        "key dimension",
        "chunk-wise kernel",
        "associative recall",
        "outer-product write",
        "read-out projection",
        "state size",
        "causal mask",
    ],
    benches: &["WikiText", "MQAR", "LAMBADA"],
    baselines: &[
        "softmax attention",
        "a Transformer++ baseline",
        "gated linear attention",
    ],
};

pub(crate) const MEMORY: Family = Family {
    name: "long-term memory",
    terms: &[
        "memory slot",
        "write policy",
        "retrieval key",
        "test-time update",
        "forgetting rate",
        "memory capacity",
        "segment summary",
        "replay buffer",
        "associative loss",
        "read gate",
    ],
    benches: &["LongRecall", "BookQA", "PassKey-Synth"],
    baselines: &[
        "a sliding-window Transformer",
        "a recurrent baseline",
        "a memory-free model",
    ],
};

pub(crate) const KV: Family = Family {
    name: "KV cache compression",
    terms: &[
        "key-value cache",
        "cache budget",
        "quantization group",
        "eviction score",
        "attention sink",
        "prefill stage",
        "decoding step",
        "head dimension",
        "cache page",
        "outlier channel",
    ],
    benches: &["LongSuite", "NeedleBench", "RULER-Synth"],
    baselines: &[
        "the full-precision cache",
        "a recent-window policy",
        "per-token quantization",
    ],
};

pub(crate) const KAN: Family = Family {
    name: "Kolmogorov-Arnold network",
    terms: &[
        "edge function",
        "spline grid",
        "basis function",
        "symbolic formula",
        "pruning threshold",
        "grid interval",
        "univariate function",
        "collocation point",
        "residual activation",
        "layer width",
    ],
    benches: &[
        "the Feynman equations",
        "a PDE suite",
        "a tabular regression suite",
    ],
    baselines: &["an MLP", "a fixed-grid KAN", "a Fourier feature network"],
};

pub(crate) const RAG: Family = Family {
    name: "retrieval-augmented generation",
    terms: &[
        "retriever",
        "reranker",
        "passage index",
        "citation",
        "query rewrite",
        "dense embedding",
        "keyword index",
        "grounding check",
        "context window",
        "chunk size",
    ],
    benches: &["CiteBench", "SciDocs-Lite", "OpenQA-Synth"],
    baselines: &["a dense retriever", "BM25", "a closed-book model"],
};

pub(crate) const MOE: Family = Family {
    name: "mixture-of-experts",
    terms: &[
        "expert",
        "router",
        "load balance",
        "top-k routing",
        "capacity factor",
        "auxiliary loss",
        "expert parallelism",
        "token dropping",
        "shared expert",
        "gating network",
    ],
    benches: &[
        "WebMix perplexity",
        "a reasoning suite",
        "a translation suite",
    ],
    baselines: &["a dense model", "switch routing", "a top-1 router"],
};

const INTRO: &[&str] = &[
    "Recent work has made the {t} a central concern, because it decides how far {f} models can scale.",
    "Despite steady progress, the interaction between the {t} and the {u} is still poorly understood.",
    "Most prior methods treat the {t} as fixed and tune the {u} around it.",
    "We argue that the {t} deserves as much attention as the {u}.",
    "This work studies {m}, a method that rethinks the {t}.",
    "A practical system has to balance the cost of the {t} against the quality of the {u}.",
    "Earlier {f} methods improved the {t} but left the {u} unchanged.",
    "Practitioners report that the {t} is the first component to fail on long inputs.",
];

const METHOD: &[&str] = &[
    "{m} computes the {t} once per layer and reuses it for every head.",
    "To keep training stable, {m} normalises the {t} before it is combined with the {u}.",
    "The {t} in {m} is parameterised by a small linear layer with {k} hidden units.",
    "We implement the {t} as a fused kernel, which avoids writing intermediate results to memory.",
    "Unlike {base}, {m} updates the {t} only when the {u} changes.",
    "Each block of {m} applies the {t} after a short convolution of width {n}.",
    "A coefficient controls how strongly the {t} depends on the {u}; we set it to 0.{n} throughout.",
    "The {t} is initialised so that {m} starts training close to {base}.",
    "Gradients through the {t} are clipped at a norm of {n}.0.",
    "During inference {m} caches the {t}, so each new token costs a constant amount of work.",
];

const EXPERIMENTS: &[&str] = &[
    "We train every model for {n}0B tokens with the same data order and report the mean of three seeds.",
    "On {b}, {m} improves over {base} by {x} points.",
    "{m} scores {pct} on {b}, compared with {pct2} for {base}.",
    "Ablating the {t} lowers the {b} score by {x} points, the largest drop among all components.",
    "Throughput is measured with a batch size of {k} on a single accelerator.",
    "The gains of {m} grow with the input length, as the {t} becomes the bottleneck.",
    "With the {u} frozen, {m} still recovers most of its improvement on {b}.",
    "Variance across seeds stays below 0.{n} points for every configuration we report.",
];

const DISCUSSION: &[&str] = &[
    "The results suggest that the {t} is the main lever for quality at a fixed budget.",
    "A limitation of {m} is that the {t} must be tuned separately for each model size.",
    "We expect the {t} to interact with the {u} at larger scales, which we leave to future work.",
    "Combining {m} with methods that target the {u} is a promising direction.",
    "Our analysis covers models up to 1.3B parameters, and larger models may behave differently.",
    "The {t} also makes {m} easier to inspect, since its behaviour can be read off per token.",
];

const SURNAMES: &[&str] = &[
    "Adeyemi",
    "Bergstrom",
    "Castillo",
    "Dubois",
    "Eriksen",
    "Fujita",
    "Gallagher",
    "Haddad",
    "Ivanova",
    "Jansen",
    "Kowalski",
    "Laurent",
    "Moreau",
    "Nakamura",
    "Okoro",
    "Petrova",
    "Quinn",
    "Rossi",
    "Sandoval",
    "Tanaka",
    "Ueda",
    "Varga",
    "Whitfield",
    "Xu",
    "Yilmaz",
    "Zielinski",
];

fn pick<'a>(rng: &mut Rng, items: &'a [&'a str]) -> &'a str {
    items[rng.below(items.len())]
}

/// `template` with its placeholders filled: `{m}` the method, `{f}` the
/// family, `{t}`/`{u}` two different terms, `{b}` a benchmark, `{base}` a
/// baseline, `{pct}`/`{pct2}` percentages, `{x}` a small decimal, `{k}` a
/// power of two, `{n}` a digit from 2 to 9.
fn fill(template: &str, method: &str, family: &Family, rng: &mut Rng) -> String {
    let t = rng.below(family.terms.len());
    let u = (t + 1 + rng.below(family.terms.len() - 1)) % family.terms.len();
    let mut out = String::with_capacity(template.len() + 32);
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let Some(len) = rest[start..].find('}') else {
            out.push_str(&rest[start..]);
            rest = "";
            break;
        };
        let key = &rest[start + 1..start + len];
        let value = match key {
            "m" => method.to_string(),
            "f" => family.name.to_string(),
            "t" => family.terms[t].to_string(),
            "u" => family.terms[u].to_string(),
            "b" => pick(rng, family.benches).to_string(),
            "base" => pick(rng, family.baselines).to_string(),
            "pct" | "pct2" => format!("{}.{}%", 40 + rng.below(55), rng.below(10)),
            "x" => format!("{}.{}", 1 + rng.below(6), rng.below(10)),
            "k" => (16usize << rng.below(6)).to_string(),
            "n" => (2 + rng.below(8)).to_string(),
            other => other.to_string(),
        };
        out.push_str(&value);
        rest = &rest[start + len + 1..];
    }
    out.push_str(rest);
    out
}

/// `count` sentences from `bank` (no template twice in one paragraph).
fn sentences(
    rng: &mut Rng,
    bank: &[&str],
    count: usize,
    method: &str,
    family: &Family,
) -> Vec<String> {
    let mut order: Vec<usize> = (0..bank.len()).collect();
    for i in (1..order.len()).rev() {
        let j = rng.below(i + 1);
        order.swap(i, j);
    }
    order
        .into_iter()
        .take(count)
        .map(|i| capitalised(&fill(bank[i], method, family, rng)))
        .collect()
}

/// A paragraph: the `lead` sentences (hand-written) followed by `count`
/// generated ones.
fn para(
    rng: &mut Rng,
    lead: &[&str],
    bank: &[&str],
    count: usize,
    method: &str,
    family: &Family,
) -> Block {
    let mut text: Vec<String> = lead.iter().map(|s| s.to_string()).collect();
    text.extend(sentences(rng, bank, count, method, family));
    Block::Paragraph(text.join(" "))
}

fn capitalised(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// `count` bibliography entries on the family's topics.
fn references(rng: &mut Rng, family: &Family, count: usize) -> Block {
    let venues = [
        "In Proceedings of the Learning Systems Conference",
        "arXiv preprint",
        "Transactions on Efficient Computing",
        "In Proceedings of the Sequence Modelling Workshop",
    ];
    let entries: Vec<String> = (1..=count)
        .map(|i| {
            let a = pick(rng, SURNAMES);
            let b = pick(rng, SURNAMES);
            let term = family.terms[rng.below(family.terms.len())];
            let venue = venues[rng.below(venues.len())];
            format!(
                "[{i}] {}. {a} and {}. {b}. {} for {} models. {venue}, {}.",
                &a[..1],
                &b[..1],
                capitalised(term),
                family.name,
                2016 + rng.below(9)
            )
        })
        .collect();
    let refs_text: Vec<&str> = entries.iter().map(String::as_str).collect();
    refs(&refs_text)
}

/// A method paper: hand-written anchors placed in fixed sections, the rest
/// generated.
pub(crate) struct MethodSpec {
    pub path: &'static str,
    pub title: &'static str,
    pub method: &'static str,
    pub family: &'static Family,
    pub authors: &'static str,
    /// Opens the abstract: what the method is.
    pub summary: &'static str,
    /// Opens the method section: how it works.
    pub mechanism: &'static str,
    pub equation: &'static str,
    /// Opens the results paragraph: the headline number.
    pub result: &'static str,
}

fn method_paper(spec: &MethodSpec) -> PdfPaper {
    let rng = &mut Rng::new(spec.path);
    let (m, f) = (spec.method, spec.family);
    let blocks = vec![
        Block::Title(spec.title.to_string()),
        Block::Authors(spec.authors.to_string()),
        h("Abstract"),
        para(rng, &[spec.summary], INTRO, 2, m, f),
        h("1 Introduction"),
        para(rng, &[], INTRO, 4, m, f),
        para(rng, &[], INTRO, 3, m, f),
        h("2 Background"),
        para(rng, &[], DISCUSSION, 2, m, f),
        para(rng, &[], METHOD, 3, m, f),
        h("3 Method"),
        h("3.1 Overview"),
        para(rng, &[spec.mechanism], METHOD, 3, m, f),
        eq(spec.equation),
        para(rng, &[], METHOD, 4, m, f),
        h("3.2 Implementation"),
        para(rng, &[], METHOD, 4, m, f),
        para(rng, &[], METHOD, 3, m, f),
        h("4 Experiments"),
        h("4.1 Setup"),
        para(rng, &[], EXPERIMENTS, 4, m, f),
        h("4.2 Results"),
        para(rng, &[spec.result], EXPERIMENTS, 3, m, f),
        para(rng, &[], EXPERIMENTS, 3, m, f),
        h("4.3 Ablations"),
        para(rng, &[], EXPERIMENTS, 4, m, f),
        h("5 Discussion"),
        para(rng, &[], DISCUSSION, 4, m, f),
        h("6 Limitations"),
        para(rng, &[], DISCUSSION, 3, m, f),
        h("References"),
        references(rng, f, 5),
    ];
    PdfPaper {
        path: spec.path,
        title: spec.title,
        blocks,
    }
}

pub const VESPER: &str = "papers/vesper-sparse-linear-attention.pdf";
pub const TESSERA: &str = "papers/tessera-state-expansion.pdf";
pub const CAIRN: &str = "papers/cairn-slot-routed-memory.pdf";
pub const MEANDER: &str = "papers/meander-episodic-replay.pdf";
pub const PRISM: &str = "papers/prism-low-rank-kv-cache.pdf";
pub const SENTINEL: &str = "papers/sentinel-kv-cache-offloading.pdf";
pub const RIVULET: &str = "papers/rivulet-fourier-kan.pdf";
pub const QUARRY: &str = "papers/quarry-kan-pde-solver.pdf";
pub const BEACON: &str = "papers/beacon-citation-grounded-rag.pdf";
pub const FATHOM: &str = "papers/fathom-hybrid-retrieval.pdf";
pub const CONFLUX: &str = "papers/conflux-expert-routing.pdf";
pub const ESTUARY: &str = "papers/estuary-expert-merging.pdf";
pub const THESIS: &str = "papers/thesis-memory-for-long-context-models.pdf";
pub const KV_SURVEY: &str = "papers/kv-cache-compression-survey.pdf";

pub(crate) const METHODS: &[MethodSpec] = &[
    MethodSpec {
        path: VESPER,
        title: "VESPER: Sparse Feature Maps for Linear Attention",
        method: "VESPER",
        family: &LINEAR,
        authors: "Ana Lucia Reyes, Henrik Dahl (Brightwater Institute)",
        summary: "VESPER keeps only the 16 largest entries of each feature vector, so a state write touches \
                  a small fraction of the state.",
        mechanism: "The sparsity pattern of VESPER is chosen by a top-k over the feature map and is recomputed \
                    for every token.",
        equation: "phi(x) = topk_16(relu(W x)),   S_t = S_{t-1} + v_t phi(k_t)^T     (1)",
        result: "At 760M parameters VESPER reaches a WikiText perplexity of 18.35 while updating its state \
                 3.4 times faster than dense linear attention.",
    },
    MethodSpec {
        path: TESSERA,
        title: "TESSERA: State Expansion for Recurrent Language Models",
        method: "TESSERA",
        family: &LINEAR,
        authors: "Kofi Asante, Maren Holt (Fjordline AI)",
        summary: "TESSERA expands the state of every head by a factor of four and contracts it again before \
                  the read-out with a learned projection.",
        mechanism: "The expansion in TESSERA is a fixed random projection of the keys, while the contraction \
                    is learned with the rest of the model.",
        equation: "S_t = S_{t-1} + v_t (P k_t)^T,   o_t = C S_t P q_t               (1)",
        result: "TESSERA lifts MQAR accuracy from 61.0 to 83.5 at 1.3B parameters.",
    },
    MethodSpec {
        path: CAIRN,
        title: "CAIRN: Associative Memory with Learned Slot Routing",
        method: "CAIRN",
        family: &MEMORY,
        authors: "Ewa Sobczak, Tomas Lindahl (Greyfield University)",
        summary: "CAIRN writes each token into the two memory slots whose keys are closest to it and leaves \
                  the other 254 slots unchanged.",
        mechanism: "Slot keys in CAIRN are learned during pretraining and stay fixed at inference, so routing \
                    a token costs one comparison per slot.",
        equation: "M_j = M_j + g_t v_t  for the 2 slots j nearest to k_t              (1)",
        result: "CAIRN reaches 58.6% accuracy on LongRecall at 64K tokens.",
    },
    MethodSpec {
        path: MEANDER,
        title: "MEANDER: Episodic Memory Replay for Long Documents",
        method: "MEANDER",
        family: &MEMORY,
        authors: "Ruth Abebe, Daniel Szabo (Kingsmoor Research)",
        summary: "MEANDER stores a summary vector at every paragraph boundary and replays the 8 most similar \
                  summaries before answering a question.",
        mechanism: "Summaries in MEANDER are the mean of the hidden states of a paragraph, projected to 256 \
                    dimensions.",
        equation: "r = sum over the top 8 i of softmax(q^T s_i) s_i                 (1)",
        result: "Replay improves accuracy on the synthetic BookQA benchmark from 39.2% to 57.8%.",
    },
    MethodSpec {
        path: PRISM,
        title: "PRISM: Low-Rank Projection of Keys for Smaller KV Caches",
        method: "PRISM",
        family: &KV,
        authors: "Lena Fischer, Arjun Menon (Coastline Compute)",
        summary: "PRISM projects every cached key onto a rank-64 subspace computed once per layer from \
                  calibration data.",
        mechanism: "The subspace of PRISM is the span of the top singular vectors of keys collected on 512 \
                    calibration sequences.",
        equation: "k_t' = U_64^T k_t,   score = q_t^T U_64 k_t'                       (1)",
        result: "PRISM reduces KV cache memory by 2.9 times on its own and by 9.8 times when combined with \
                 two-bit quantization.",
    },
    MethodSpec {
        path: SENTINEL,
        title: "SENTINEL: Offloading the KV Cache to Host Memory",
        method: "SENTINEL",
        family: &KV,
        authors: "Grace Owusu, Pavel Novak (Riverbend Systems)",
        summary: "SENTINEL keeps the cache in host memory and prefetches the pages that the previous layer's \
                  queries attended to most.",
        mechanism: "Prefetching in SENTINEL overlaps the transfer of layer l + 1 with the attention of layer \
                    l, hiding most of the transfer time.",
        equation: "pages(l + 1) = top_32 pages by sum of attention in layer l        (1)",
        result: "SENTINEL serves 1M-token contexts on a single 24 GB GPU at 9.6 tokens per second.",
    },
    MethodSpec {
        path: RIVULET,
        title: "RIVULET: Fourier Kolmogorov-Arnold Networks",
        method: "RIVULET",
        family: &KAN,
        authors: "Sienna Marsh, Oleksandr Bondar (Westgate Mathematics Lab)",
        summary: "RIVULET replaces the B-splines of a KAN with truncated Fourier series of 12 terms on every \
                  edge.",
        mechanism: "The frequencies of RIVULET are fixed integers and only the coefficients of each edge are \
                    trained.",
        equation: "phi(x) = sum_{j=1..12} a_j cos(j x) + b_j sin(j x)                (1)",
        result: "RIVULET trains 2.7 times faster per step than a spline KAN of the same width.",
    },
    MethodSpec {
        path: QUARRY,
        title: "QUARRY: Kolmogorov-Arnold Networks as PDE Solvers",
        method: "QUARRY",
        family: &KAN,
        authors: "Matteo Ricci, Yasmin Haddad (Eastfield Applied Mathematics)",
        summary: "QUARRY trains a Kolmogorov-Arnold network with a physics-informed loss that adds the squared \
                  PDE residual at 4,096 collocation points.",
        mechanism: "Boundary conditions in QUARRY are imposed exactly by multiplying the network output with a \
                    function that vanishes on the boundary.",
        equation: "loss = mean_i (Laplace u(x_i) - f(x_i))^2                       (1)",
        result: "QUARRY solves the Poisson equation on the unit square with a relative L2 error of 3.4e-4.",
    },
    MethodSpec {
        path: BEACON,
        title: "BEACON: Citation-Grounded Retrieval-Augmented Generation",
        method: "BEACON",
        family: &RAG,
        authors: "Nora Lindberg, Samuel Okafor (Harbourview AI)",
        summary: "BEACON asks the generator to cite a passage for every sentence and drops sentences whose \
                  cited passage does not entail them.",
        mechanism: "Entailment in BEACON is checked by a small cross-encoder that reads the sentence and its \
                    cited passage together.",
        equation: "keep(s) = 1[ p_entail(s, passage(s)) > 0.5 ]                    (1)",
        result: "BEACON raises citation precision from 61.4% to 88.0% on the synthetic CiteBench.",
    },
    MethodSpec {
        path: FATHOM,
        title: "FATHOM: Hybrid Sparse-Dense Retrieval for Scientific Papers",
        method: "FATHOM",
        family: &RAG,
        authors: "Iris Navarro, Bjorn Aalto (Northgate Information Lab)",
        summary: "FATHOM fuses a BM25 ranking and a dense ranking with reciprocal rank fusion using a constant \
                  of 60.",
        mechanism: "FATHOM indexes the title and abstract of each paper as a separate field and boosts matches \
                    in them by a factor of two.",
        equation: "score(d) = 1 / (60 + rank_bm25(d)) + 1 / (60 + rank_dense(d))     (1)",
        result: "FATHOM improves recall at 10 from 0.71 to 0.84 on the synthetic SciDocs-Lite benchmark.",
    },
    MethodSpec {
        path: CONFLUX,
        title: "CONFLUX: Load-Balanced Routing for Mixture-of-Experts",
        method: "CONFLUX",
        family: &MOE,
        authors: "Helena Sousa, Wei Jian (Summit Lane Research)",
        summary: "CONFLUX routes every token to 2 of 64 experts and adds an auxiliary loss that penalises \
                  uneven expert load.",
        mechanism: "The router of CONFLUX adds a learned bias per expert that is raised for under-used experts \
                    between training steps.",
        equation: "loss_aux = 64 * sum_e f_e p_e                                   (1)",
        result: "CONFLUX matches a dense 1.3B model's perplexity with 31% of its training compute.",
    },
    MethodSpec {
        path: ESTUARY,
        title: "ESTUARY: Merging Experts after Training",
        method: "ESTUARY",
        family: &MOE,
        authors: "Freya Jonsson, Ibrahim Diallo (Lakeside Machine Learning)",
        summary: "ESTUARY merges experts whose weight vectors have a cosine similarity above 0.9 into a single \
                  expert.",
        mechanism: "Merged experts in ESTUARY take the average of their weights, weighted by how many tokens \
                    each expert received.",
        equation: "W_merged = (n_a W_a + n_b W_b) / (n_a + n_b)                      (1)",
        result: "Merging reduces the number of experts from 64 to 41 with a perplexity increase of 0.12.",
    },
];

/// One catalogue entry as a long review cites it: name, year and result
/// only (the mechanism stays in the first survey, which the questions ask
/// about).
fn cited_result(prefix: &str, (name, year, _mechanism, result): &Entry) -> String {
    format!("{prefix} {name} ({year}): {result}")
}

/// A chapter or section of generated discussion: `paragraphs` paragraphs of
/// the given banks.
fn discussion(
    rng: &mut Rng,
    blocks: &mut Vec<Block>,
    paragraphs: usize,
    banks: &[&[&str]],
    method: &str,
    family: &Family,
) {
    for i in 0..paragraphs {
        let bank = banks[i % banks.len()];
        blocks.push(para(rng, &[], bank, 4, method, family));
    }
}

fn thesis() -> PdfPaper {
    let rng = &mut Rng::new(THESIS);
    let mut blocks = vec![
        Block::Title(
            "Remembering at Scale: Memory Mechanisms for Long-Context Language Models".to_string(),
        ),
        Block::Authors(
            "Hana Kowalczyk. Doctoral thesis, Department of Computing, Ashbury University"
                .to_string(),
        ),
        h("Abstract"),
        p("In this thesis, a memory mechanism is any component that carries information from one token \
           to a later one without recomputing it. We study four kinds: recurrent states, learned memories, \
           compressed key-value caches and retrieval from an external index. The central claim of this \
           thesis is that write policies matter more than memory size: what a model decides to store \
           predicts its long-context accuracy better than how much it can store."),
    ];
    let chapters: [(&str, &Family, &[Entry]); 6] = [
        ("Recurrent States", &LINEAR, LINEAR_FAMILY),
        ("Learned Memories", &MEMORY, MEMORY_FAMILY),
        ("Compressed Key-Value Caches", &KV, QUANT_FAMILY),
        ("Evicting Cached Tokens", &KV, EVICTION_FAMILY),
        ("Retrieval from an External Index", &RAG, &[]),
        ("Sparse Experts as Memory", &MOE, &[]),
    ];
    for (n, (name, family, entries)) in chapters.iter().enumerate() {
        let chapter = n + 1;
        let topic = family.name;
        blocks.push(h(&format!("{chapter} {name}")));
        blocks.push(h(&format!("{chapter}.1 Background")));
        discussion(rng, &mut blocks, 4, &[INTRO, DISCUSSION], topic, family);
        blocks.push(h(&format!("{chapter}.2 Mechanisms")));
        discussion(rng, &mut blocks, 4, &[METHOD], topic, family);
        if !entries.is_empty() {
            blocks.push(h(&format!("{chapter}.3 Methods compared in this chapter")));
            for entry in entries.iter() {
                blocks.push(Block::Paragraph(cited_result(
                    "We reproduce the reported result of",
                    entry,
                )));
            }
        }
        blocks.push(h(&format!("{chapter}.4 Experiments")));
        discussion(rng, &mut blocks, 4, &[EXPERIMENTS], topic, family);
        blocks.push(h(&format!("{chapter}.5 Summary")));
        discussion(rng, &mut blocks, 2, &[DISCUSSION], topic, family);
    }
    blocks.extend([
        h("7 Conclusion"),
        p("Across six chapters the same pattern holds: models that choose what to write, through gates, \
           surprise, eviction scores or routing, beat larger memories that store everything. We therefore \
           recommend reporting the write policy of a memory mechanism alongside its size."),
        h("References"),
        references(rng, &MEMORY, 12),
    ]);
    PdfPaper {
        path: THESIS,
        title: "Remembering at Scale: Memory Mechanisms for Long-Context Language Models",
        blocks,
    }
}

fn kv_survey() -> PdfPaper {
    let rng = &mut Rng::new(KV_SURVEY);
    let topic = "KV cache compression";
    let mut blocks = vec![
        Block::Title("KV Cache Compression for Long-Context Inference: A Survey".to_string()),
        Block::Authors(
            "Olga Mikhailova, Tunde Bakare, Ines Carvalho (Institute for Serving Systems)".to_string(),
        ),
        h("Abstract"),
        p("The key-value cache of a 70B model at 128K tokens exceeds 300 GB in half precision, which makes \
           it the main obstacle to long-context serving. We survey methods that shrink it and divide KV \
           cache compression into token-level, precision-level and dimension-level methods."),
        h("1 Introduction"),
    ];
    discussion(rng, &mut blocks, 5, &[INTRO], topic, &KV);
    blocks.push(h("2 Token-Level Methods"));
    discussion(rng, &mut blocks, 5, &[METHOD, DISCUSSION], topic, &KV);
    for entry in EVICTION_FAMILY {
        blocks.push(Block::Paragraph(cited_result("Reported result for", entry)));
    }
    blocks.push(h("3 Precision-Level Methods"));
    discussion(rng, &mut blocks, 5, &[METHOD, DISCUSSION], topic, &KV);
    for entry in QUANT_FAMILY {
        blocks.push(Block::Paragraph(cited_result("Reported result for", entry)));
    }
    blocks.push(h("4 Dimension-Level Methods"));
    discussion(rng, &mut blocks, 5, &[METHOD, EXPERIMENTS], topic, &KV);
    blocks.push(h("5 Systems Support"));
    discussion(rng, &mut blocks, 5, &[METHOD, DISCUSSION], topic, &KV);
    blocks.push(h("6 Evaluation Practice"));
    discussion(rng, &mut blocks, 5, &[EXPERIMENTS], topic, &KV);
    blocks.extend([
        h("7 Outlook"),
        p("Token-level and precision-level methods are now mature, and the open questions concern how to \
           combine them with offloading and with models whose recurrent state already replaces part of \
           the cache."),
        h("References"),
        references(rng, &KV, 10),
    ]);
    PdfPaper {
        path: KV_SURVEY,
        title: "KV Cache Compression for Long-Context Inference: A Survey",
        blocks,
    }
}

/// Reading notes: two per topic, generated text only (no question is about
/// them). They bring the library to more files than the rerank pool can hold
/// at two candidates each, the case in which a per-file pool limit could
/// crowd out the chunks a question about one paper needs.
pub(crate) const NOTES: &[(&str, &str, &Family)] = &[
    (
        "notes/linear-attention-notes-1.pdf",
        "linear attention",
        &LINEAR,
    ),
    (
        "notes/linear-attention-notes-2.pdf",
        "linear attention",
        &LINEAR,
    ),
    (
        "notes/long-term-memory-notes-1.pdf",
        "long-term memory",
        &MEMORY,
    ),
    (
        "notes/long-term-memory-notes-2.pdf",
        "long-term memory",
        &MEMORY,
    ),
    ("notes/kv-cache-notes-1.pdf", "KV cache compression", &KV),
    ("notes/kv-cache-notes-2.pdf", "KV cache compression", &KV),
    ("notes/kan-notes-1.pdf", "Kolmogorov-Arnold networks", &KAN),
    ("notes/kan-notes-2.pdf", "Kolmogorov-Arnold networks", &KAN),
    (
        "notes/rag-notes-1.pdf",
        "retrieval-augmented generation",
        &RAG,
    ),
    (
        "notes/rag-notes-2.pdf",
        "retrieval-augmented generation",
        &RAG,
    ),
    ("notes/moe-notes-1.pdf", "mixture-of-experts", &MOE),
    ("notes/moe-notes-2.pdf", "mixture-of-experts", &MOE),
];

fn note(path: &'static str, topic: &str, family: &Family) -> PdfPaper {
    let rng = &mut Rng::new(path);
    let mut blocks = vec![
        Block::Title(format!("Reading group notes: {topic}")),
        Block::Authors("Reading group of the Ashbury machine learning lab".to_string()),
        h("1 Background"),
    ];
    discussion(rng, &mut blocks, 3, &[INTRO, DISCUSSION], topic, family);
    blocks.push(h("2 Methods we read"));
    discussion(rng, &mut blocks, 3, &[METHOD], topic, family);
    blocks.push(h("3 Results we discussed"));
    discussion(rng, &mut blocks, 3, &[EXPERIMENTS], topic, family);
    blocks.push(h("4 Open questions"));
    discussion(rng, &mut blocks, 2, &[DISCUSSION], topic, family);
    PdfPaper {
        path,
        title: "Reading group notes",
        blocks,
    }
}

/// The generated papers: method papers, the thesis, the KV cache survey and
/// the reading notes.
pub(crate) fn pdf_papers() -> Vec<PdfPaper> {
    let mut papers: Vec<PdfPaper> = METHODS.iter().map(method_paper).collect();
    papers.push(thesis());
    papers.push(kv_survey());
    papers.extend(
        NOTES
            .iter()
            .map(|&(path, topic, family)| note(path, topic, family)),
    );
    papers
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_fill_every_placeholder_deterministically() {
        let mut a = Rng::new("seed");
        let mut b = Rng::new("seed");
        for template in INTRO
            .iter()
            .chain(METHOD)
            .chain(EXPERIMENTS)
            .chain(DISCUSSION)
        {
            let x = fill(template, "ZETA", &KV, &mut a);
            let y = fill(template, "ZETA", &KV, &mut b);
            assert_eq!(x, y);
            assert!(!x.contains('{') && !x.contains('}'), "{x}");
            assert!(x.is_ascii(), "{x}");
        }
        let mut c = Rng::new("other seed");
        assert_ne!(Rng::new("seed").next(), c.next());
        assert!((0..100).all(|_| c.below(7) < 7));
    }

    #[test]
    fn two_terms_of_one_sentence_differ() {
        let mut rng = Rng::new("terms");
        for _ in 0..200 {
            let s = fill("{t}|{u}", "M", &LINEAR, &mut rng);
            let (t, u) = s.split_once('|').unwrap();
            assert_ne!(t, u);
        }
    }
}
