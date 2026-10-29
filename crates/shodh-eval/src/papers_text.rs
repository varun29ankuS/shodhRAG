//! The text of the synthetic paper corpus ([`crate::papers`]) and its
//! questions. Every method, author, benchmark and number is invented.

use crate::papers::{eq, fig, h, p, refs, table, Block, PdfPaper, Question};

pub const LUMEN: &str = "papers/lumen-gated-delta-rule.pdf";
pub const TIDE: &str = "papers/tide-test-time-memory.pdf";
pub const SQUEEZE: &str = "papers/squeeze-kv-cache-quantization.pdf";
pub const HARBOR: &str = "papers/harbor-kv-cache-eviction.pdf";
pub const KANDLE: &str = "papers/kandle-kolmogorov-arnold-networks.pdf";
pub const SURVEY: &str = "papers/efficient-sequence-models-survey.pdf";
pub const ORBIT: &str = "papers/orbit-rotary-chunkwise-attention.tex";

fn title(text: &str) -> Block {
    Block::Title(text.to_string())
}

fn authors(text: &str) -> Block {
    Block::Authors(text.to_string())
}

fn lumen() -> PdfPaper {
    PdfPaper {
        path: LUMEN,
        title: "LUMEN: Linear Attention with a Gated Delta Rule",
        blocks: vec![
            title("LUMEN: Linear Attention with a Gated Delta Rule"),
            authors("Mira Okonkwo, Tarun Vaidya, Elsa Lindqvist (Halvard Institute)"),
            h("Abstract"),
            p("Linear attention replaces the softmax with a kernel feature map so that a model keeps a \
               fixed-size state instead of a growing key-value cache. Its weakness is recall: the state \
               only ever accumulates, so old associations interfere with new ones. We present LUMEN, a \
               linear attention layer whose state is updated by a gated delta rule. A scalar gate decays \
               the state and a delta step overwrites the value stored under the current key. LUMEN \
               trains in parallel over chunks, runs as a recurrence at inference, and matches a \
               Transformer of the same size in perplexity while improving associative recall."),
            h("1 Introduction"),
            p("Softmax attention compares every query with every key, so its cost grows quadratically, \
               as O(n^2) in the sequence length n, and its key-value cache grows linearly with the context. \
               Linear attention removes both costs by writing each key-value pair into a matrix-valued \
               state and reading it with the query. The price is that the state has a fixed capacity and \
               no way to remove what it stored, which shows up as poor recall on long inputs."),
            p("Two remedies have been studied separately. Decay gates shrink the whole state at every \
               step, which frees capacity but forgets uniformly. The delta rule instead replaces the value \
               stored under a key, which edits memory precisely but never frees space. LUMEN combines the \
               two in one update and shows that the combination is better than either alone."),
            p("Our contributions are threefold. First, we define the gated delta rule and show that it is a \
               single step of online gradient descent on a regression loss with weight decay. Second, we \
               derive a chunk-wise parallel form that trains efficiently on accelerators. Third, we train \
               models up to 1.3B parameters and report language modelling and recall results."),
            h("2 Background: Linear Attention"),
            p("A causal linear attention layer keeps a state S_t, a matrix with one row per value \
               dimension and one column per key dimension. At each step it adds the outer product of \
               the value and the key and reads the output with the query:"),
            eq("S_t = S_{t-1} + v_t k_t^T,    o_t = S_t q_t                    (1)"),
            p("This recurrence has constant memory per token. Because nothing is ever removed, the \
               state saturates once the number of stored pairs exceeds the key dimension, and retrieval \
               of an early key returns a mixture of everything written since."),
            h("3 Method"),
            h("3.1 The Gated Delta Rule"),
            p("The delta rule writes the difference between the new value and the value currently \
               retrieved for the key, scaled by a learned writing strength beta_t. We add a gate alpha_t \
               in (0, 1), computed from the input token, that multiplies the state before the write:"),
            eq("S_t = alpha_t S_{t-1} (I - beta_t k_t k_t^T) + beta_t v_t k_t^T   (2)"),
            p("In this update the gate alpha_t decays the whole state before the delta update writes the \
               new association, so the layer can clear its memory at a document boundary while still \
               editing single associations inside a document. With alpha_t fixed at 1 the update reduces \
               to the plain delta rule, and with beta_t fixed at 0 it reduces to a decaying state."),
            p("Equation (2) is one step of online gradient descent with learning rate beta_t on the loss \
               ||S k_t - v_t||^2, followed by weight decay with factor alpha_t. Keys are L2-normalised so \
               that the update is a contraction and training stays stable."),
            h("3.2 Chunk-wise Parallel Form"),
            p("Running Equation (2) token by token is sequential and slow on accelerators. We split the \
               sequence into chunks and compute each chunk's contribution with matrix multiplications, \
               passing only the state from one chunk to the next. The products of the delta terms inside \
               a chunk are kept in a compact WY representation, so no d by d matrix is formed per token. \
               In all experiments we use a chunk size of 64 tokens, which balanced memory and speed on \
               our hardware."),
            h("4 Experiments"),
            p("We train models of 340M and 1.3B parameters on 100B tokens of the synthetic WebMix corpus \
               with the same data order for every architecture. We report perplexity on WikiText, \
               accuracy on LAMBADA, and accuracy on the MQAR associative recall task with 256 key-value \
               pairs."),
            table(
                "Table 2: Language modelling and recall at 1.3B parameters.",
                &["Model", "Params", "Wiki ppl", "LAMBADA", "MQAR"],
                &[
                    &["Transformer++", "1.3B", "16.85", "51.9", "88.4"],
                    &["Linear attention", "1.3B", "19.73", "44.1", "21.6"],
                    &["Delta rule", "1.3B", "17.40", "49.8", "70.2"],
                    &["LUMEN", "1.3B", "16.42", "52.8", "79.5"],
                ],
            ),
            p("LUMEN improves WikiText perplexity over the Transformer++ baseline and closes most of the \
               recall gap that plain linear attention leaves. The gate and the delta step are both needed: \
               removing the gate costs 0.98 perplexity, and removing the delta step costs 58 points of \
               MQAR accuracy."),
            fig("Figure 2: Training throughput at 16K context. LUMEN processes 41K tokens per second per GPU, \
                 2.3 times the FlashAttention-2 baseline, and its advantage grows with the context length."),
            h("5 Limitations"),
            p("The fixed-size state remains a bottleneck for tasks that must reproduce long inputs verbatim: \
               exact copying of long spans remains weaker than in softmax attention, and in our tests a \
               hybrid with two softmax layers was needed to match it. We also have not studied models above \
               1.3B parameters."),
            h("References"),
            refs(&[
                "[1] J. Halloran and P. Ibe. Fast weight memories for sequence learning. Journal of Neural \
                 Recurrence, 1993.",
                "[2] K. Marr, S. Oduya and T. Brandt. Kernelized attention with feature maps. In Proceedings \
                 of the Learning Systems Conference, 2020.",
                "[3] R. Castellan. The delta rule revisited for linear transformers. arXiv preprint, 2021.",
                "[4] A. Petrov and L. Huang. Decay gates for recurrent attention. In Proceedings of the \
                 Sequence Modelling Workshop, 2023.",
            ]),
        ],
    }
}

fn tide() -> PdfPaper {
    PdfPaper {
        path: TIDE,
        title: "TIDE: Learning to Memorize at Test Time with Surprise-Gated Updates",
        blocks: vec![
            title("TIDE: Learning to Memorize at Test Time with Surprise-Gated Updates"),
            authors("Sofia Brandauer, Kwame Mensah, Lin Qiaoyu (Northfield Lab for Machine Learning)"),
            h("Abstract"),
            p("Attention is an excellent short-term memory but cannot scale to millions of tokens. We \
               introduce TIDE, a neural long-term memory module that keeps learning while the model is \
               being used. The memory is a small multilayer perceptron whose weights are updated at test \
               time by gradient descent on an associative loss, so that it memorizes the tokens that \
               surprise it most. TIDE reaches 71.3% accuracy on LongRecall at 16K tokens and scales to \
               inputs of two million tokens."),
            h("1 Introduction"),
            p("Models with a fixed state, such as linear attention, compress the past into a matrix and \
               lose detail as the context grows. Models with attention keep every token but pay for it in \
               memory. Test-time training offers a third option: treat memorization as learning, and let a \
               parametric memory adapt to each input as it streams by."),
            p("We take inspiration from human memory, where events that violate expectations are \
               remembered better. In TIDE, surprise is the gradient of the associative loss with respect \
               to the memory parameters: a token whose value the memory predicts badly produces a large \
               gradient and changes the memory a lot, while an expected token leaves it almost untouched."),
            h("2 The Memory Module"),
            p("The memory M is a two-layer perceptron that maps keys to values. Each incoming token is \
               projected to a key k_t and a value v_t, and the memory is trained online to minimise the \
               reconstruction error of the pair:"),
            eq("loss(M; x_t) = || M(k_t) - v_t ||^2                            (1)"),
            p("A single gradient step per token would react only to the present token. Instead, momentum \
               carries the surprise of earlier tokens, so that the tokens following a surprising event are \
               also memorized even when each of them looks unremarkable on its own. The update with \
               momentum eta_t and step size theta_t is:"),
            eq("U_t = eta_t U_{t-1} - theta_t grad loss(M_{t-1}; x_t)              (2)"),
            eq("M_t = (1 - a_t) M_{t-1} + U_t                                  (3)"),
            p("Old information is removed by a data-dependent forgetting gate a_t in Equation (3). When the \
               input changes topic the gate approaches one and clears most of the memory; within a topic it \
               stays near zero. All three coefficients eta_t, theta_t and a_t are predicted from the token \
               by small linear layers."),
            h("3 Adding Memory to a Model"),
            p("We compare memory as context, memory as gate and memory as layer. As context, the memory's \
               output for the current segment is prepended to the attention window. As a gate, the memory \
               and a sliding-window attention run in parallel and their outputs are mixed. As a layer, the \
               memory is stacked in front of the attention blocks."),
            p("Training uses a parallel form: the sequence is split into segments of 512 tokens, the \
               gradients inside a segment are computed with respect to the memory at the segment start, \
               and momentum is accumulated with a parallel scan."),
            h("4 Experiments"),
            p("We evaluate on the synthetic LongRecall benchmark, which hides 64 facts in filler text and \
               asks for one of them, and on language modelling with the WebMix corpus. All models have \
               760M parameters."),
            table(
                "Table 3: LongRecall accuracy (%) by context length.",
                &["Model", "4K", "16K", "64K", "256K"],
                &[
                    &["Transformer (window 4K)", "92.1", "24.0", "6.2", "1.5"],
                    &["Linear attention", "58.4", "31.7", "12.9", "4.1"],
                    &["TIDE as context", "93.0", "71.3", "60.8", "48.2"],
                    &["TIDE as layer", "90.6", "66.5", "55.1", "41.0"],
                ],
            ),
            p("Memory as context is the strongest variant. Removing momentum lowers accuracy at 16K tokens \
               from 71.3% to 52.9%, and removing the forgetting gate makes training unstable beyond 64K \
               tokens."),
            h("5 Conclusion"),
            p("Treating memory as a model that learns at test time gives long-context recall that neither \
               attention nor fixed-state recurrences reach at this scale. The cost is a slower forward pass: \
               TIDE is 1.4 times slower than a Transformer of the same size at 4K tokens."),
            h("References"),
            refs(&[
                "[1] M. Okonkwo, T. Vaidya and E. Lindqvist. LUMEN: Linear attention with a gated delta rule. \
                 In Proceedings of the Learning Systems Conference, 2024.",
                "[2] D. Farrow and N. Iyer. Test-time training layers for sequence models. arXiv preprint, \
                 2024.",
                "[3] H. Lindgren. Surprise and memory consolidation. Cognitive Systems Review, 2019.",
            ]),
        ],
    }
}

fn squeeze() -> PdfPaper {
    PdfPaper {
        path: SQUEEZE,
        title: "SQUEEZE: Two-Bit Key-Value Cache Quantization with Per-Channel Keys",
        blocks: vec![
            title("SQUEEZE: Two-Bit Key-Value Cache Quantization with Per-Channel Keys"),
            authors("Priya Raman, Oskar Weiss, Dana Feld (Corvid Systems Research)"),
            h("Abstract"),
            p("During long-context inference the key-value cache, not the model weights, dominates GPU \
               memory. We present SQUEEZE, a tuning-free method that stores the cache in two bits per \
               number. Keys are quantized per channel and values per token, following the structure of \
               their outliers, and a small window of recent tokens is kept in full precision. SQUEEZE \
               reduces KV cache memory by 5.3 times and raises decoding throughput by 3.1 times with an \
               average accuracy loss of 0.4 points on LongSuite."),
            h("1 Introduction"),
            p("For a 7B model serving a batch of 16 requests at 32K tokens, the cache needs 256 GB in half \
               precision, far more than the weights. Quantizing the cache is attractive because it needs no \
               retraining, but naive two-bit quantization destroys accuracy. We analyse why and design a \
               quantizer around what we find."),
            h("2 Analysis of the Cache"),
            p("We inspected the cached keys and values of every layer of three model families. Keys and \
               values behave differently. In the keys, a few key channels carry outliers whose magnitude is \
               ten to fifty times the median, and they are the same channels for every token. Values show no \
               fixed outlier channels, but their error matters per token because the attention output is a \
               weighted sum of value rows."),
            p("This motivates quantizing keys per channel, so that each outlier channel gets its own scale, \
               and quantizing values per token, so that the error of each value row is bounded. Quantizing \
               keys per token, as earlier work did, spreads one outlier's range over the whole row and leaves \
               the other channels with almost no resolution."),
            h("3 Method"),
            p("Each quantization group shares a scale and a zero point. We use a group size of 32 for both \
               keys and values and asymmetric rounding to two bits:"),
            eq("Q(x) = round((x - z) / s),  s = (max - min) / 3,  z = min           (1)"),
            p("Per-channel quantization of keys needs a group of tokens to be complete before it can be \
               quantized, so the newest tokens wait in a residual buffer: the most recent 128 tokens stay in \
               full precision and are quantized together once the buffer is full. Attention is computed \
               over the quantized part and the residual part and the two results are combined."),
            h("4 Results"),
            table(
                "Table 1: Memory, throughput and LongSuite accuracy, 7B model, 32K context.",
                &["Method", "Bits", "Memory", "Throughput", "LongSuite"],
                &[
                    &["Full precision", "16", "1.0x", "1.0x", "47.6"],
                    &["Per-token 2-bit", "2", "5.3x", "3.1x", "31.2"],
                    &["SQUEEZE", "2", "5.3x", "3.1x", "47.2"],
                ],
            ),
            p("SQUEEZE keeps accuracy within 0.4 points of full precision on average while using a fifth of \
               the memory, and the larger batches it allows raise throughput by 3.1 times. The residual \
               buffer is essential: without it accuracy drops by 2.7 points on the retrieval tasks."),
            h("5 Discussion"),
            p("Quantization reduces the size of every cached entry but keeps every entry. It is therefore \
               complementary to methods that decide which tokens to keep, and the two can be applied \
               together."),
            h("References"),
            refs(&[
                "[1] G. Alvarez and M. Stone. Outlier channels in transformer activations. Transactions on \
                 Efficient Computing, 2022.",
                "[2] Y. Kato. Asymmetric low-bit rounding for inference. arXiv preprint, 2023.",
            ]),
        ],
    }
}

fn harbor() -> PdfPaper {
    PdfPaper {
        path: HARBOR,
        title: "HARBOR: Heavy-Hitter Aware Eviction for Long-Context KV Caches",
        blocks: vec![
            title("HARBOR: Heavy-Hitter Aware Eviction for Long-Context KV Caches"),
            authors("Nadia Ferreira, Jonas Ekholm (Lattice AI Lab)"),
            h("Abstract"),
            p("Most cached tokens are rarely attended to. HARBOR is a training-free eviction policy that \
               keeps only a fixed budget of the key-value cache: the first tokens of the sequence, a window \
               of recent tokens, and the heavy hitters that have received the most attention so far. At a \
               cache budget of 20% HARBOR keeps 97.8% of the full-cache accuracy on LongSuite."),
            h("1 Introduction"),
            p("Attention in trained language models is sparse: in a typical layer, five percent of the \
               tokens receive more than half of the attention mass. If the tokens that matter can be \
               identified while decoding, the rest of the cache can be dropped without retraining."),
            h("2 Method"),
            p("HARBOR always keeps the first four tokens because they act as attention sinks: the softmax \
               must place its mass somewhere, and models learn to park it on the first positions, so \
               evicting them changes every later attention distribution. It also keeps the 256 most recent \
               tokens, which carry the local context."),
            p("The remaining budget goes to heavy hitters. Each cached token keeps a running sum of the \
               attention it has received from all queries so far, and HARBOR ranks tokens by this \
               accumulated attention score. When the cache is full the token with the lowest score outside \
               the sinks and the recent window is evicted."),
            eq("A_j = sum over t of softmax(q_t K^T)_j,   evict argmin A_j         (1)"),
            h("3 Results"),
            table(
                "Table 1: LongSuite accuracy by cache budget, 7B model.",
                &["Policy", "Budget", "LongSuite", "Kept"],
                &[
                    &["Full cache", "100%", "47.6", "100%"],
                    &["Recent window only", "20%", "33.0", "69.3%"],
                    &["HARBOR", "20%", "46.6", "97.8%"],
                    &["HARBOR + SQUEEZE", "20%", "46.1", "96.8%"],
                ],
            ),
            p("Eviction and quantization address different redundancy and combine well: applying HARBOR on \
               top of SQUEEZE two-bit quantization reduces cache memory by 12 times with a loss of 1.5 \
               points."),
            h("4 Limitations"),
            p("An evicted token cannot be recovered. Questions about a detail that received little attention \
               early in a long document are the main failure case, which a quantized cache does not have."),
            h("References"),
            refs(&[
                "[1] P. Raman, O. Weiss and D. Feld. SQUEEZE: Two-bit key-value cache quantization with \
                 per-channel keys. arXiv preprint, 2024.",
                "[2] T. Okafor and R. Lind. Attention sinks in long sequences. In Proceedings of the \
                 Sequence Modelling Workshop, 2023.",
            ]),
        ],
    }
}

fn kandle() -> PdfPaper {
    PdfPaper {
        path: KANDLE,
        title: "KANDLE: Grid-Adaptive Kolmogorov-Arnold Networks",
        blocks: vec![
            title("KANDLE: Grid-Adaptive Kolmogorov-Arnold Networks"),
            authors("Ilse Vermeer, Rohan Kapoor, Yuki Sato (Meridian Mathematics Group)"),
            h("Abstract"),
            p("Kolmogorov-Arnold networks (KANs) replace the fixed activation functions of a multilayer \
               perceptron with learnable univariate functions. We present KANDLE, a KAN whose B-spline \
               activations refine their grids during training. On the Feynman equations benchmark KANDLE \
               reaches a median RMSE of 1.2e-4 with ten times fewer parameters than an MLP of similar \
               accuracy."),
            h("1 Introduction"),
            p("KANs are based on the Kolmogorov-Arnold representation theorem, which states that every \
               continuous multivariate function on a bounded domain can be written as a finite composition \
               of continuous functions of one variable and addition. A KAN layer generalises this \
               construction to arbitrary widths and depths."),
            p("In a multilayer perceptron the nodes apply fixed nonlinearities and the edges carry scalar \
               weights. In a KAN the learnable activation functions sit on edges and the nodes only sum their \
               inputs, so every edge is a small function that can be plotted and, often, recognised."),
            h("2 Method"),
            p("Each edge function is a cubic B-spline plus a residual SiLU term. The spline has order k = 3 \
               and G grid intervals over the range of its input:"),
            eq("phi(x) = w_b silu(x) + w_s sum_i c_i B_i(x)                     (1)"),
            p("Coarse grids train quickly but limit accuracy, and fine grids from the start train slowly. \
               KANDLE uses grid extension from G = 5 to G = 20 in three stages: when the loss plateaus, each \
               spline is refitted by least squares on a finer grid, so training continues from the coarse \
               solution instead of restarting."),
            p("After training, edges whose L1 norm falls below a threshold are pruned, and the remaining \
               edge functions can be matched against a library of symbolic forms such as sin, exp and x^2."),
            h("3 Results"),
            table(
                "Table 1: Median test RMSE on the Feynman equations benchmark.",
                &["Model", "Parameters", "RMSE"],
                &[
                    &["MLP (4 layers)", "12,400", "3.1e-4"],
                    &["KAN (fixed grid G = 5)", "1,180", "9.6e-4"],
                    &["KANDLE (G = 5 to 20)", "1,240", "1.2e-4"],
                ],
            ),
            p("Grid extension accounts for most of the improvement over a fixed-grid KAN. Training is slower \
               per step than for an MLP, by a factor of about 6, because each edge evaluates a spline."),
            h("References"),
            refs(&[
                "[1] V. Arnoldsen. On the representation of continuous functions of several variables. \
                 Mathematical Notes, 1957.",
                "[2] C. de Boor-Lind. A practical guide to splines for learning. Numerical Methods Press, \
                 2001.",
            ]),
        ],
    }
}

/// A method in the survey's catalogue: name, year, mechanism, result.
type Entry = (&'static str, u32, &'static str, &'static str);

const LINEAR_FAMILY: &[Entry] = &[
    ("Alder", 2021, "It replaces the softmax with a positive random feature map so that attention can be computed as a running sum.", "It reports a WikiText perplexity of 21.4 at 350M parameters."),
    ("Basalt", 2022, "It adds a learned scalar decay to every head so that the state forgets at a rate chosen per head.", "It reports a WikiText perplexity of 19.8 at 350M parameters."),
    ("Cobalt", 2022, "It normalises the read-out by the running sum of keys, which keeps the output scale stable over long inputs.", "Its authors measure a 1.9 times speedup over softmax attention at 8K tokens."),
    ("Drift", 2023, "It shifts each token's key by one position before writing it, giving the state a cheap form of local order.", "It improves MQAR recall by 6 points over Alder."),
    ("Ember", 2023, "It writes keys and values into a state whose rows are sorted by a hash of the key, so similar keys share rows.", "It reports a LAMBADA accuracy of 45.0 at 760M parameters."),
    ("Fennel", 2024, "It mixes a short convolution into the key projection so that n-gram statistics reach the state.", "It reaches a WikiText perplexity of 18.1 at 760M parameters."),
    ("Garnet", 2024, "It uses data-dependent gates on both the rows and the columns of the state, at a cost of one extra projection.", "It reports a WikiText perplexity of 17.6 at 1.3B parameters."),
    ("Jasper", 2024, "It keeps two states with different decay rates and lets each head read a learned mixture of them.", "It improves MQAR recall by 9 points over Basalt."),
    ("Kestrel", 2024, "It writes only every second token into the state and interpolates the reads in between, halving the update cost.", "It trains 1.6 times faster than Fennel at equal perplexity."),
    ("Lichen", 2025, "It replaces the outer-product write with a low-rank update whose rank grows with the length of the input.", "It reports a WikiText perplexity of 17.1 at 1.3B parameters."),
    ("Moss", 2025, "It applies a learned orthogonal transition to the state before each write, so information rotates rather than decays.", "It keeps 80% MQAR recall at 512 key-value pairs."),
];

const MEMORY_FAMILY: &[Entry] = &[
    ("Heron", 2020, "It keeps an external memory of past segment summaries that later segments read with cross-attention.", "It improves LongRecall accuracy at 64K tokens to 35.0%."),
    ("Iris", 2021, "It compresses each segment into eight memory slots with a learned pooling layer before discarding the tokens.", "It reports 28.4% LongRecall accuracy at 64K tokens."),
    ("Juniper", 2022, "It trains a fast-weight matrix with a Hebbian rule inside each layer and resets it at document boundaries.", "It is 1.2 times slower than a Transformer at 4K tokens."),
    ("Kelp", 2023, "It stores key-value pairs in a differentiable hash table that grows by doubling when it is three quarters full.", "It keeps 90% recall up to 128K tokens on its own benchmark."),
    ("Larch", 2023, "It updates a small recurrent network by one gradient step per segment, an early form of test-time training.", "It reports 41.7% LongRecall accuracy at 64K tokens."),
    ("Marrow", 2024, "It mixes a sliding-window attention with a recurrent summary through a learned scalar gate per channel.", "It reaches a WikiText perplexity of 18.9 at 760M parameters."),
    ("Nimbus", 2024, "It routes each token either to a memory write or to a skip path, chosen by a learned router.", "It halves the number of memory writes with no loss in recall."),
    ("Nettle", 2024, "It keeps a sparse set of memory slots addressed by learned keys and writes each token into its two nearest slots.", "It reports 44.9% LongRecall accuracy at 64K tokens."),
    ("Osprey", 2025, "It trains its memory with a second-order optimiser at inference, using a diagonal estimate of the curvature.", "It reports 63.0% LongRecall accuracy at 64K tokens."),
    ("Plover", 2025, "It stores a compressed copy of every thousandth token alongside a recurrent summary of the tokens in between.", "It keeps 85% recall up to one million tokens on its own benchmark."),
    ("Quartz", 2025, "It shares one memory across the layers of a model and lets each layer write to its own region of it.", "It uses 30% fewer parameters than a per-layer memory of equal recall."),
];

const QUANT_FAMILY: &[Entry] = &[
    ("Opal", 2022, "It stores the cache in eight-bit integers with one scale per attention head.", "It halves cache memory with no measurable accuracy loss."),
    ("Pumice", 2023, "It quantizes keys and values per token to four bits and keeps the attention sink tokens in full precision.", "It reduces cache memory by 3.6 times."),
    ("Quill", 2023, "It applies a random rotation to keys before quantizing them, which spreads outliers across channels.", "It loses 1.1 points on LongSuite at three bits."),
    ("Ripple", 2024, "It quantizes the cache progressively, lowering the precision of tokens as they age from eight bits to two.", "It reduces cache memory by 4.4 times with a loss of 0.9 points on LongSuite."),
    ("Sorrel", 2024, "It shares one set of quantized keys across adjacent layers whose keys are highly correlated.", "It reduces cache memory by 6.0 times."),
    ("Tamarind", 2024, "It learns a vector codebook per layer and stores each cached key as the index of its nearest code.", "It reaches 1.5 bits per number with a loss of 2.3 points."),
    ("Umber", 2025, "It mixes precisions per head, giving more bits to heads whose attention is spread over many tokens.", "It loses 0.6 points on LongSuite at an average of 2.5 bits."),
    ("Velvet", 2025, "It quantizes values with a per-token scale and stores the scales themselves in eight bits.", "It reduces cache memory by 4.8 times."),
    ("Willow", 2025, "It keeps keys in four bits and values in two, after finding that attention scores are more sensitive to key error.", "It loses 0.5 points on LongSuite."),
    ("Yew", 2025, "It compresses the cache along the head dimension with a learned projection before quantizing it.", "It reduces cache memory by 7.1 times with a loss of 1.8 points."),
    ("Zinnia", 2025, "It decides the precision of each block of tokens from the norm of its keys, spending bits on the largest keys.", "It reaches an average of 2.2 bits per number."),
];

const EVICTION_FAMILY: &[Entry] = &[
    ("Vetch", 2022, "It evicts tokens uniformly at random once the cache is full, a baseline that is surprisingly hard to beat on short tasks.", "It keeps 81% of full-cache accuracy at a 50% budget."),
    ("Wren", 2023, "It keeps a fixed window of recent tokens plus the first few tokens of the sequence.", "It keeps 72% of full-cache accuracy at a 20% budget."),
    ("Xylem", 2023, "It merges evicted tokens into their most similar kept neighbour instead of dropping them.", "It keeps 93% of full-cache accuracy at a 20% budget."),
    ("Yarrow", 2024, "It chooses the budget per layer from the entropy of the layer's attention, giving more cache to diffuse layers.", "It keeps 95% of full-cache accuracy at a 15% budget."),
    ("Zephyr", 2024, "It scores tokens with an observation window of the last 32 queries before generation starts.", "It keeps 96% of full-cache accuracy at a 10% budget on retrieval tasks."),
    ("Aster", 2025, "It offloads evicted entries to CPU memory and fetches them back when a query attends strongly to their page.", "It recovers 99% of full-cache accuracy at a 10% GPU budget."),
    ("Bracken", 2025, "It predicts future attention with a small learned probe and evicts the tokens the probe expects to be ignored.", "It keeps 97% of full-cache accuracy at a 15% budget."),
    ("Cedar", 2025, "It evicts whole pages of sixteen tokens at a time, which keeps the cache layout friendly to paged attention kernels.", "It keeps 94% of full-cache accuracy at a 20% budget."),
    ("Dahlia", 2025, "It evicts different tokens in different heads, so that each head keeps the tokens it attends to most.", "It keeps 96% of full-cache accuracy at a 15% budget."),
    ("Elm", 2025, "It shrinks the budget of deeper layers, where attention concentrates on fewer tokens.", "It keeps 95% of full-cache accuracy at an average budget of 12%."),
    ("Fern", 2025, "It re-scores the cache only every 64 decoding steps, which removes most of the cost of tracking attention.", "It decodes 1.3 times faster than per-step scoring at equal accuracy."),
];

const HYBRID_FAMILY: &[Entry] = &[
    ("Cress", 2023, "It interleaves one softmax attention layer after every six linear attention layers.", "It matches Transformer recall on MQAR at 1.3B parameters."),
    ("Dune", 2024, "It runs a state space layer and a sliding-window attention in parallel in every block and adds their outputs.", "It reaches a WikiText perplexity of 16.9 at 1.3B parameters."),
    ("Egret", 2024, "It uses global attention only in the middle third of the layers and linear attention elsewhere.", "It reduces cache memory by 3.0 times relative to a Transformer."),
    ("Flint", 2024, "It shares a single key-value cache across all of its softmax layers, which are spaced eight layers apart.", "It reduces cache memory by 8.0 times."),
    ("Gorse", 2025, "It replaces the feed-forward block of every fourth layer with a Kolmogorov-Arnold layer.", "It reports a WikiText perplexity of 17.2 at 760M parameters."),
    ("Hazel", 2025, "It distils a pretrained Transformer into a linear attention model by matching attention maps layer by layer.", "It recovers 96% of the teacher's accuracy with 2B training tokens."),
    ("Ibex", 2025, "It decides per token whether to attend with softmax or with the recurrent state, using a learned threshold.", "It uses softmax attention for 12% of tokens on average."),
    ("Jacaranda", 2025, "It runs a test-time trained memory in parallel with a linear attention layer and gates between them per token.", "It reports 66.4% LongRecall accuracy at 64K tokens."),
    ("Kumquat", 2025, "It stores the key-value cache of its few softmax layers in two bits while its linear layers keep a state.", "It reduces cache memory by 21 times relative to a Transformer."),
    ("Lotus", 2025, "It places a sliding-window attention of 2K tokens in every layer and a global memory in every eighth layer.", "It reaches a WikiText perplexity of 16.6 at 1.3B parameters."),
    ("Mallow", 2025, "It converts a pretrained hybrid into a pure recurrent model for deployment by distilling its attention layers.", "It keeps 94% of the hybrid's accuracy."),
];

/// One catalogue paragraph, in one of three wordings so that entries do not
/// read as copies of each other.
fn catalogue_paragraph(i: usize, family: &str, (name, year, mechanism, result): &Entry) -> Block {
    let text = match i % 3 {
        0 => format!("{name} ({year}) belongs to the {family} family. {mechanism} {result}"),
        1 => format!(
            "Within the {family} line of work, {name} ({year}) is notable for one design choice. \
             {mechanism} {result}"
        ),
        _ => format!("{name}, published in {year}, is a {family} method. {mechanism} {result}"),
    };
    Block::Paragraph(text)
}

fn catalogue(blocks: &mut Vec<Block>, heading: &str, family: &str, intro: &str, entries: &[Entry]) {
    blocks.push(h(heading));
    blocks.push(p(intro));
    for (i, entry) in entries.iter().enumerate() {
        blocks.push(catalogue_paragraph(i, family, entry));
    }
}

fn survey() -> PdfPaper {
    let mut blocks = vec![
        title("Efficient Sequence Models: A Survey of Linear Attention, Test-Time Memory and KV Cache \
               Compression"),
        authors("Amara Diallo, Felix Grunewald, Mei Tanaka, Lucas Moreau (Collegium for Efficient AI)"),
        h("Abstract"),
        p("Long-context language models are limited by the cost of attention and by the memory of the \
           key-value cache. This survey reviews three responses to these limits: linear attention and other \
           fixed-state recurrences, memory modules that learn while the model runs, and compression of the \
           key-value cache by quantization and eviction. We organise more than forty methods into a common \
           taxonomy, compare their reported results, and list open problems."),
        h("1 Introduction"),
        p("A Transformer decoding a long input pays twice. Each new token attends to every previous token, \
           and every previous token must be kept in the key-value cache. For long documents, code bases and \
           agent transcripts the cache, rather than the weights, determines how many requests fit on an \
           accelerator and how long each of them may be."),
        p("Work on this problem has been fragmented across communities. Architecture papers replace \
           attention with recurrences that keep a fixed-size state, systems papers compress the cache of an \
           unchanged model, and a newer line of work treats memory as a model that is trained at inference \
           time. These directions are rarely compared with each other, and their benchmarks differ."),
        p("This survey brings them together. Section 2 introduces a taxonomy. Sections 3 to 5 review each \
           family with a catalogue of representative methods, Section 6 discusses hybrid designs, Section 7 \
           compares reported results and Section 8 lists open problems."),
        h("2 Taxonomy"),
        p("We group efficient sequence models into three families according to what they change. The first \
           family changes the attention operator itself so that the past is summarised in a state of fixed \
           size. The second family adds a memory that is written and read by learning rather than by \
           storage. The third family leaves the model unchanged and reduces the cache, either by storing each \
           entry in fewer bits or by storing fewer entries."),
        p("The families trade different resources. Fixed-state recurrences save both computation and memory \
           but lose precise recall. Learned memories recover recall at the cost of extra computation during \
           inference. Cache compression keeps the original model's quality best but saves only memory, not \
           the quadratic attention computation."),
        h("3 Linear Attention and Fixed-State Recurrences"),
        p("Linear attention writes each key-value pair into a matrix-valued state and reads it with the \
           query, so the cost of each step is constant. The early variants simply summed the outer products, \
           and the state saturated once it held as many pairs as its key dimension. Two ideas made later \
           models competitive: decay gates, which shrink the state at every step, and the delta rule, which \
           overwrites the value stored for a key."),
        p("LUMEN [12] combines both ideas in a gated delta rule, in which a scalar gate multiplies the state \
           before a delta step writes the new association. It trains with a chunk-wise parallel algorithm and \
           reports a WikiText perplexity of 16.42 at 1.3B parameters, below a Transformer baseline of the \
           same size. ORBIT [13] adds rotary position encoding to chunk-wise linear attention and keeps the \
           training cost linear in the sequence length."),
    ];
    catalogue(
        &mut blocks,
        "3.1 Catalogue of linear attention variants",
        "linear attention",
        "The following methods are representative of the linear attention family. Each entry gives the \
         main idea and the headline result reported by its authors; numbers are not directly comparable \
         across entries because training data and model sizes differ.",
        LINEAR_FAMILY,
    );
    blocks.extend([
        h("4 Memory Modules that Learn at Inference"),
        p("A fixed-size state must decide at write time what to keep. Learned memories instead fit a small \
           network to the stream of keys and values while the model runs, so that capacity is spent where \
           the input is hard to predict. This idea, known as test-time training, turns memorization into an \
           optimisation problem solved during inference."),
        p("TIDE [14] is the most complete example. Its memory is a two-layer perceptron trained by gradient \
           descent on a key-to-value reconstruction loss, with momentum and a forgetting gate, and tokens \
           that the memory predicts badly change it the most. Used as extra context for attention, it reports \
           71.3% accuracy on LongRecall at 16K tokens."),
    ]);
    catalogue(
        &mut blocks,
        "4.1 Catalogue of memory modules",
        "recurrent memory",
        "The following methods add an explicit or learned memory to a sequence model. They differ in what \
         is stored, when it is written, and whether the memory is trained in advance or at inference.",
        MEMORY_FAMILY,
    );
    blocks.extend([
        h("5 Compressing the Key-Value Cache"),
        p("Cache compression leaves the trained model untouched and reduces the memory of its key-value \
           cache. Quantization keeps every cached token but stores each number in fewer bits. Eviction keeps \
           full precision but stores only a budget of tokens, chosen by recency, position or the attention \
           they receive."),
        p("Among quantization methods, SQUEEZE [15] stores the cache in two bits, quantizing keys per channel \
           because of their fixed outlier channels and values per token, and reports a 5.3 times memory \
           reduction. Among eviction methods, HARBOR [16] keeps attention sinks, a recent window and the \
           tokens with the highest accumulated attention, and keeps 97.8% of full-cache accuracy at a 20% \
           budget. The two are complementary, and HARBOR's authors report a 12 times reduction when they are \
           applied together."),
    ]);
    catalogue(
        &mut blocks,
        "5.1 Catalogue of cache quantization methods",
        "cache quantization",
        "The following methods reduce the number of bits per cached number. Most of them are training-free \
         and differ in how they group numbers for a shared scale and how they treat outliers.",
        QUANT_FAMILY,
    );
    catalogue(
        &mut blocks,
        "5.2 Catalogue of cache eviction methods",
        "cache eviction",
        "The following methods keep a subset of the cached tokens. They differ in the score that ranks tokens \
         and in how the budget is spread across layers and heads.",
        EVICTION_FAMILY,
    );
    blocks.extend([
        h("6 Hybrid Designs"),
        p("Several recent models combine families: a few softmax attention layers restore precise recall \
           in an otherwise linear model, or a recurrent summary extends a sliding-window attention. Hybrids \
           currently give the best quality per unit of memory, at the cost of more complex kernels and \
           serving systems."),
    ]);
    catalogue(
        &mut blocks,
        "6.1 Catalogue of hybrid models",
        "hybrid",
        "The following models mix attention with recurrent or learned components in one network.",
        HYBRID_FAMILY,
    );
    blocks.extend([
        h("7 Comparison of Reported Results"),
        p("Reported results are hard to compare: papers train on different data, at different sizes, and \
           evaluate on different benchmarks. Where a common benchmark exists the picture is consistent. On \
           language modelling, gated recurrences now match Transformers of the same size. On recall at long \
           contexts, learned memories and hybrids lead, while pure fixed-state models fall behind beyond 16K \
           tokens. Cache compression methods lose less than one point of accuracy down to about two bits or \
           a 20% budget, and degrade quickly below that."),
        table(
            "Table 4: Headline results reported by the methods discussed in Sections 3 to 5.",
            &["Method", "Family", "Benchmark", "Result"],
            &[
                &["LUMEN", "linear", "WikiText ppl", "16.42"],
                &["TIDE", "memory", "LongRecall 16K", "71.3%"],
                &["SQUEEZE", "quantization", "Memory saved", "5.3x"],
                &["HARBOR", "eviction", "Accuracy kept", "97.8%"],
            ],
        ),
        h("8 Open Problems"),
        p("We see four open problems. First, there is no shared long-context benchmark on which \
           architectures and cache compression methods are evaluated together. Second, the interaction \
           between compression and learned memory is unexplored: a model with test-time memory may need a \
           smaller cache, but no work measures this. Third, eviction errors cannot be undone, and methods \
           that can recover evicted entries cheaply are only beginning to appear. Fourth, efficiency is \
           reported in incomparable units, and we recommend reporting memory per token and tokens per \
           second at a fixed context length."),
        h("9 Conclusion"),
        p("Efficient sequence modelling has moved from approximating attention to redesigning memory. The \
           most promising systems combine a compact recurrent state, a small amount of exact attention and \
           a compressed cache, and the field needs common benchmarks to tell which combination is best."),
        h("References"),
        refs(&[
            "[1] K. Marr, S. Oduya and T. Brandt. Kernelized attention with feature maps. In Proceedings of \
             the Learning Systems Conference, 2020.",
            "[2] R. Castellan. The delta rule revisited for linear transformers. arXiv preprint, 2021.",
            "[3] A. Petrov and L. Huang. Decay gates for recurrent attention. In Proceedings of the Sequence \
             Modelling Workshop, 2023.",
            "[4] D. Farrow and N. Iyer. Test-time training layers for sequence models. arXiv preprint, 2024.",
            "[5] T. Okafor and R. Lind. Attention sinks in long sequences. In Proceedings of the Sequence \
             Modelling Workshop, 2023.",
            "[6] G. Alvarez and M. Stone. Outlier channels in transformer activations. Transactions on \
             Efficient Computing, 2022.",
            "[12] M. Okonkwo, T. Vaidya and E. Lindqvist. LUMEN: Linear attention with a gated delta rule. In \
             Proceedings of the Learning Systems Conference, 2024.",
            "[13] B. Achterberg and S. Moreno. ORBIT: Rotary position encoding for chunk-wise linear \
             attention. arXiv preprint, 2024.",
            "[14] S. Brandauer, K. Mensah and L. Qiaoyu. TIDE: Learning to memorize at test time with \
             surprise-gated updates. arXiv preprint, 2025.",
            "[15] P. Raman, O. Weiss and D. Feld. SQUEEZE: Two-bit key-value cache quantization with \
             per-channel keys. arXiv preprint, 2024.",
            "[16] N. Ferreira and J. Ekholm. HARBOR: Heavy-hitter aware eviction for long-context KV caches. \
             arXiv preprint, 2024.",
        ]),
    ]);
    PdfPaper {
        path: SURVEY,
        title: "Efficient Sequence Models: A Survey of Linear Attention, Test-Time Memory and KV Cache Compression",
        blocks,
    }
}

const ORBIT_TEX: &str = r"\documentclass{article}
\usepackage{amsmath}
\title{ORBIT: Rotary Position Encoding for Chunk-wise Linear Attention}
\author{Bram Achterberg \and Sara Moreno}
\begin{document}
\maketitle

\begin{abstract}
Linear attention has no notion of token order beyond what its gates provide. We present ORBIT, which applies rotary position encoding to the queries and keys of chunk-wise linear attention without giving up its parallel training. Training cost stays linear in the sequence length, and ORBIT improves length extrapolation from 2K to 32K tokens.
\end{abstract}

\section{Introduction}
Softmax attention with rotary position encoding is the standard way to give a Transformer a sense of relative position. Linear attention cannot use it directly, because the rotation of a key depends on its absolute position and is folded into the state when the key is written.

ORBIT applies the rotation to queries and keys before the feature map and stores rotated keys in the state. Inside a chunk, relative positions are exact; across chunks, the state carries keys rotated by their absolute position, and the query is rotated by its own position, so their product depends only on the distance between them.

\section{Method}
For a chunk of length $C$ starting at position $s$ we write
\begin{equation}
\mathbf{S}_{s+C} = \mathbf{S}_{s} + \sum_{i=s}^{s+C-1} \mathbf{v}_i \, (\mathbf{R}_{i} \mathbf{k}_i)^\top ,
\end{equation}
where $\mathbf{S}_t$ is the state matrix after token t and $\mathbf{R}_i$ is the rotation matrix for position $i$. The output of a query combines an intra-chunk term computed with masked attention and an inter-chunk term read from the state.

The intra-chunk term costs $O(C^2 d)$ and the inter-chunk term $O(C d^2)$ per chunk, so for a fixed chunk size the training complexity is linear in the sequence length. We use a chunk size of 128 tokens.

\section{Experiments}
We train 760M-parameter models on sequences of 2K tokens and evaluate perplexity at up to 32K tokens. Without position encoding, perplexity at 32K tokens rises to 31.5; with ORBIT it stays at 18.7, close to its value of 17.9 at 2K tokens.

\begin{thebibliography}{9}
\bibitem{rope} L. Sun and W. Chao. Rotary position embedding for transformers. arXiv preprint, 2021.
\bibitem{lumen} M. Okonkwo, T. Vaidya and E. Lindqvist. LUMEN: Linear attention with a gated delta rule. In Proceedings of the Learning Systems Conference, 2024.
\bibitem{chunk} H. Varga. Chunk-wise training of recurrent attention. arXiv preprint, 2023.
\end{thebibliography}
\end{document}
";

pub(crate) fn pdf_papers() -> Vec<PdfPaper> {
    vec![lumen(), tide(), squeeze(), harbor(), kandle(), survey()]
}

pub(crate) fn text_papers() -> Vec<(&'static str, &'static str)> {
    vec![(ORBIT, ORBIT_TEX)]
}

macro_rules! q {
    ($id:literal, $question:literal, [$(($file:expr, $passage:literal)),+ $(,)?], [$($fact:literal),* $(,)?]) => {
        Question {
            id: $id,
            question: $question,
            sources: &[$(($file, $passage)),+],
            facts: &[$($fact),*],
        }
    };
}

pub(crate) fn questions() -> Vec<Question> {
    vec![
        // Definitions and mechanisms, one paper each.
        q!("lumen-gate", "What does the gate alpha_t do in the gated delta rule?",
           [(LUMEN, "the gate alpha_t decays the whole state before the delta update writes the new association")], ["decays the whole state"]),
        q!("lumen-equation", "What is the state update equation of LUMEN?",
           [(LUMEN, "S_t = alpha_t S_{t-1} (I - beta_t k_t k_t^T) + beta_t v_t k_t^T")], []),
        q!("lumen-chunk-size", "What chunk size does LUMEN use for chunk-wise parallel training?",
           [(LUMEN, "we use a chunk size of 64 tokens")], ["64 tokens"]),
        q!("lumen-gradient-view", "Why is the gated delta rule a form of online gradient descent?",
           [(LUMEN, "one step of online gradient descent with learning rate beta_t on the loss")], ["weight decay"]),
        q!("lumen-quadratic", "Why does softmax attention cost O(n^2) in the sequence length?",
           [(LUMEN, "Softmax attention compares every query with every key")], []),
        q!("lumen-limitation", "What limitation of LUMEN do its authors acknowledge?",
           [(LUMEN, "exact copying of long spans remains weaker than in softmax attention")], ["copying"]),
        q!("tide-surprise", "How does TIDE measure surprise?",
           [(TIDE, "surprise is the gradient of the associative loss with respect to the memory parameters")], ["gradient"]),
        q!("tide-momentum", "Why does TIDE use momentum when it updates its memory?",
           [(TIDE, "momentum carries the surprise of earlier tokens")], []),
        q!("tide-forgetting", "How does TIDE remove old information from its memory?",
           [(TIDE, "Old information is removed by a data-dependent forgetting gate")], ["forgetting gate"]),
        q!("tide-variants", "In which ways can the TIDE memory be added to a model?",
           [(TIDE, "We compare memory as context, memory as gate and memory as layer")], ["memory as context", "memory as gate", "memory as layer"]),
        q!("tide-loss", "What loss does the TIDE memory minimise at test time?",
           [(TIDE, "loss(M; x_t) = || M(k_t) - v_t ||^2")], []),
        q!("tide-test-time", "What is test-time training and why use it for memory?",
           [(TIDE, "Test-time training offers a third option: treat memorization as learning")], []),
        q!("squeeze-per-channel", "Why does SQUEEZE quantize keys per channel?",
           [(SQUEEZE, "a few key channels carry outliers whose magnitude is ten to fifty times the median")], ["outliers"]),
        q!("squeeze-residual", "How many recent tokens does SQUEEZE keep in full precision?",
           [(SQUEEZE, "the most recent 128 tokens stay in full precision")], ["128"]),
        q!("squeeze-group", "What quantization group size does SQUEEZE use?",
           [(SQUEEZE, "We use a group size of 32 for both keys and values")], ["32"]),
        q!("squeeze-cache-size", "How large is the KV cache of a 7B model serving 16 requests at 32K tokens?",
           [(SQUEEZE, "the cache needs 256 GB in half precision")], ["256 GB"]),
        q!("harbor-sinks", "Why does HARBOR always keep the first tokens of the sequence?",
           [(HARBOR, "because they act as attention sinks")], ["attention sinks"]),
        q!("harbor-heavy-hitters", "How does HARBOR rank tokens to find heavy hitters?",
           [(HARBOR, "HARBOR ranks tokens by this accumulated attention score")], ["accumulated attention"]),
        q!("harbor-failure", "When does KV cache eviction with HARBOR fail?",
           [(HARBOR, "Questions about a detail that received little attention early in a long document are the main failure case")], []),
        q!("kandle-theorem", "Which theorem are Kolmogorov-Arnold networks based on?",
           [(KANDLE, "KANs are based on the Kolmogorov-Arnold representation theorem")], ["Kolmogorov-Arnold representation theorem"]),
        q!("kandle-edges", "Where do the learnable activation functions sit in a KAN?",
           [(KANDLE, "the learnable activation functions sit on edges and the nodes only sum their inputs")], ["edges"]),
        q!("kandle-grid", "How does KANDLE refine its spline grid during training?",
           [(KANDLE, "KANDLE uses grid extension from G = 5 to G = 20 in three stages")], ["G = 20"]),
        q!("kandle-spline-order", "What spline order do KANDLE's edge functions use?",
           [(KANDLE, "The spline has order k = 3")], ["k = 3"]),
        q!("orbit-rotation", "How does ORBIT make rotary position encoding work with linear attention?",
           [(ORBIT, "ORBIT applies the rotation to queries and keys before the feature map and stores rotated keys in the state")], []),
        q!("orbit-state-symbol", "What does \\mathbf{S}_t denote in the ORBIT paper?",
           [(ORBIT, "is the state matrix after token t")], ["state matrix"]),
        q!("survey-taxonomy", "What taxonomy does the survey use to group efficient sequence models?",
           [(SURVEY, "We group efficient sequence models into three families according to what they change")], ["three families"]),
        q!("survey-open-problems", "What open problems remain for efficient long-context models?",
           [(SURVEY, "We see four open problems")], ["benchmark"]),
        q!("survey-ripple", "What does Ripple do to the KV cache?",
           [(SURVEY, "lowering the precision of tokens as they age from eight bits to two")], ["eight bits to two"]),
        // Tables and figures.
        q!("lumen-table-ppl", "What WikiText perplexity does LUMEN reach at 1.3B parameters?",
           [(LUMEN, "LUMEN 1.3B 16.42 52.8 79.5")], ["16.42"]),
        q!("lumen-figure-throughput", "How much faster than FlashAttention-2 is LUMEN's training at 16K context?",
           [(LUMEN, "2.3 times the FlashAttention-2 baseline")], ["2.3 times"]),
        q!("tide-table-16k", "What LongRecall accuracy does TIDE reach at 16K tokens?",
           [(TIDE, "TIDE as context 93.0 71.3 60.8 48.2")], ["71.3"]),
        q!("squeeze-table", "What LongSuite score does SQUEEZE keep with 2-bit quantization?",
           [(SQUEEZE, "SQUEEZE 2 5.3x 3.1x 47.2")], ["47.2"]),
        q!("kandle-table-rmse", "What RMSE does KANDLE reach on the Feynman equations benchmark?",
           [(KANDLE, "KANDLE (G = 5 to 20) 1,240 1.2e-4")], ["1.2e-4"]),
        // "Which paper" questions: the paper itself, not the works that cite it.
        q!("which-gated-delta", "Which paper proposed the gated delta rule for linear attention?",
           [(LUMEN, "We present LUMEN, a linear attention layer whose state is updated by a gated delta rule")], ["LUMEN"]),
        q!("which-per-channel", "Which paper introduced per-channel quantization of keys in the KV cache?",
           [(SQUEEZE, "Keys are quantized per channel and values per token")], ["SQUEEZE"]),
        q!("which-surprise", "Which paper uses surprise to decide what a memory should store at test time?",
           [(TIDE, "so that it memorizes the tokens that surprise it most")], ["TIDE"]),
        q!("which-heavy-hitters", "Which paper evicts KV cache entries by their accumulated attention?",
           [(HARBOR, "HARBOR is a training-free eviction policy")], ["HARBOR"]),
        q!("which-rotary", "Which paper adds rotary position encoding to chunk-wise linear attention?",
           [(ORBIT, "We present ORBIT, which applies rotary position encoding to the queries and keys of chunk-wise linear attention")], ["ORBIT"]),
        // Citations.
        q!("orbit-cites", "What work does the ORBIT paper cite for rotary position embedding?",
           [(ORBIT, "Rotary position embedding for transformers")], ["Sun"]),
        // Questions answered by several papers.
        q!("multi-kv-compare", "How do SQUEEZE and HARBOR shrink the KV cache in different ways?",
           [(SQUEEZE, "Quantization reduces the size of every cached entry but keeps every entry"),
            (HARBOR, "keeps only a fixed budget of the key-value cache")], []),
        q!("multi-kv-combine", "Can KV cache quantization and eviction be combined, and how much memory does that save?",
           [(HARBOR, "reduces cache memory by 12 times with a loss of 1.5 points"),
            (SQUEEZE, "the two can be applied together")], ["12 times"]),
        q!("multi-lumen-tide", "How do LUMEN and TIDE differ in the way they write to memory?",
           [(LUMEN, "The delta rule writes the difference between the new value and the value currently retrieved for the key"),
            (TIDE, "the memory is trained online to minimise the reconstruction error of the pair")], []),
        q!("multi-long-context-memory", "What approaches in my papers reduce the memory needed for long-context inference?",
           [(SQUEEZE, "reduces KV cache memory by 5.3 times"),
            (HARBOR, "keeps only a fixed budget of the key-value cache"),
            (LUMEN, "keeps a fixed-size state instead of a growing key-value cache"),
            (SURVEY, "This survey reviews three responses to these limits")], []),
        q!("multi-chunkwise", "Which methods train linear attention with a chunk-wise parallel form?",
           [(LUMEN, "We split the sequence into chunks and compute each chunk's contribution with matrix multiplications"),
            (ORBIT, "The output of a query combines an intra-chunk term computed with masked attention and an inter-chunk term read from the state")], []),
        q!("multi-forgetting", "How do these models forget or drop old context?",
           [(LUMEN, "the gate alpha_t decays the whole state before the delta update writes the new association"),
            (TIDE, "Old information is removed by a data-dependent forgetting gate"),
            (HARBOR, "When the cache is full the token with the lowest score outside the sinks and the recent window is evicted")], []),
        q!("multi-recall-results", "How well do fixed-state models recall facts in long inputs compared with attention?",
           [(LUMEN, "closes most of the recall gap that plain linear attention leaves"),
            (TIDE, "Linear attention 58.4 31.7 12.9 4.1")], []),
        q!("multi-training-free", "Which KV cache methods need no retraining of the model?",
           [(SQUEEZE, "a tuning-free method that stores the cache in two bits per number"),
            (HARBOR, "HARBOR is a training-free eviction policy")], []),
        q!("multi-recurrent-state", "Which papers describe a recurrent state updated token by token?",
           [(LUMEN, "This recurrence has constant memory per token"),
            (TIDE, "the memory is trained online to minimise the reconstruction error of the pair"),
            (ORBIT, "the state carries keys rotated by their absolute position")], []),
    ]
}

pub(crate) fn unanswerable_questions() -> Vec<(&'static str, &'static str)> {
    vec![
        (
            "none-bleu",
            "What BLEU score does LUMEN reach on English-German translation?",
        ),
        ("none-funding", "Which agency funded the TIDE project?"),
        (
            "none-images",
            "How does SQUEEZE perform on image classification?",
        ),
        ("none-cluster", "How many GPUs were used to train KANDLE?"),
    ]
}
