//! The committed synthetic corpus and its dataset, generated from the fixed
//! content below (no randomness): fictional contracts and invoices as paged
//! PDFs with tables, research notes with Markdown tables, and plain-text
//! policies. Every name, number and identifier is invented.
//!
//! `shodh-eval synth --out <dir>` writes `<dir>/corpus/**` and
//! `<dir>/dataset.json`; a unit test checks that the committed copy in
//! `crates/shodh-eval/data/synthetic` is byte-identical to this generator's
//! output and that every expected passage and fact is in its source.

use std::collections::BTreeSet;
use std::path::Path;

use anyhow::{bail, Context, Result};

use crate::dataset::{Dataset, EvalCase, ExpectedSource};
use crate::pdf::{write_pdf, PdfPage, TextRun};

pub const DATASET_ID: &str = "synthetic-v1";

/// Longest line of PDF prose; keeps every line on the page at 10 pt.
const MAX_LINE_CHARS: usize = 100;

/// One generated file and its text by page (one entry for unpaged formats).
#[derive(Debug, Clone)]
pub struct SynthFile {
    pub path: String,
    pub bytes: Vec<u8>,
    pub pages: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct SynthCorpus {
    pub files: Vec<SynthFile>,
    pub dataset: Dataset,
}

enum Block {
    Heading(&'static str),
    Line(&'static str),
    /// Rows of cells at fixed column positions; the first row is bold when
    /// `header` is set.
    Table {
        header: bool,
        rows: Vec<[&'static str; 4]>,
    },
    Gap,
}

use Block::{Gap, Heading, Line};

fn table(header: Option<[&'static str; 4]>, rows: &[[&'static str; 4]]) -> Block {
    Block::Table {
        header: header.is_some(),
        rows: header.into_iter().chain(rows.iter().copied()).collect(),
    }
}

const COLUMNS: [f32; 4] = [72.0, 300.0, 370.0, 470.0];
const TOP: f32 = 730.0;

/// Lay out one page; returns the page and its text.
fn render_page(blocks: &[Block]) -> (PdfPage, String) {
    let mut runs = Vec::new();
    let mut lines: Vec<String> = Vec::new();
    let mut y = TOP;
    let mut run = |x: f32, y: f32, size: f32, bold: bool, text: &str| {
        runs.push(TextRun {
            x,
            y,
            size,
            bold,
            text: text.to_string(),
        });
    };
    for block in blocks {
        match block {
            Heading(text) => {
                run(72.0, y, 12.0, true, text);
                lines.push(text.to_string());
                y -= 20.0;
            }
            Line(text) => {
                run(72.0, y, 10.0, false, text);
                lines.push(text.to_string());
                y -= 14.0;
            }
            Block::Table { header, rows } => {
                for (i, row) in rows.iter().enumerate() {
                    let bold = *header && i == 0;
                    for (cell, x) in row.iter().zip(COLUMNS) {
                        if !cell.is_empty() {
                            run(x, y, 9.5, bold, cell);
                        }
                    }
                    let cells: Vec<&str> = row.iter().copied().filter(|c| !c.is_empty()).collect();
                    lines.push(cells.join(" "));
                    y -= 14.0;
                }
            }
            Gap => y -= 10.0,
        }
    }
    (PdfPage { runs }, lines.join("\n"))
}

fn pdf_file(path: &str, title: &str, pages: Vec<Vec<Block>>) -> SynthFile {
    let rendered: Vec<(PdfPage, String)> = pages.iter().map(|p| render_page(p)).collect();
    let pdf_pages: Vec<PdfPage> = rendered.iter().map(|(p, _)| p.clone()).collect();
    SynthFile {
        path: path.to_string(),
        bytes: write_pdf(title, &pdf_pages),
        pages: rendered.into_iter().map(|(_, text)| text).collect(),
    }
}

fn text_file(path: &str, body: &str) -> SynthFile {
    SynthFile {
        path: path.to_string(),
        bytes: body.as_bytes().to_vec(),
        pages: vec![body.to_string()],
    }
}

const INVOICE_HEADER: [&str; 4] = ["Item", "Qty", "Rate (INR)", "Amount (INR)"];

fn documents() -> Vec<SynthFile> {
    vec![
        pdf_file(
            "contracts/msa-northwind-bluepeak.pdf",
            "Master Services Agreement: Northwind Analytics and Bluepeak Systems",
            vec![
                vec![
                    Heading("MASTER SERVICES AGREEMENT"),
                    Line("This Master Services Agreement is made between Northwind Analytics Pvt. Ltd. (the Client)"),
                    Line("and Bluepeak Systems LLP (the Service Provider)."),
                    Line("The Effective Date of this Agreement is 1 April 2025."),
                    Gap,
                    Heading("1. Scope of Services"),
                    Line("The Service Provider will design, build and operate a data warehouse for the Client."),
                    Line("Each engagement is described in a Statement of Work signed by both parties."),
                    Gap,
                    Heading("2. Term"),
                    Line("This Agreement has an initial term of thirty-six (36) months from the Effective Date."),
                    Line("It renews automatically for successive twelve (12) month periods unless terminated."),
                ],
                vec![
                    Heading("3. Fees and Payment"),
                    Line("The Client shall pay each undisputed invoice within forty-five (45) days of receipt."),
                    Line("Late payments accrue interest at 1.5% per month on the overdue amount."),
                    Line("The monthly operations retainer is INR 2,75,000 plus applicable GST."),
                    Gap,
                    Heading("4. Limitation of Liability"),
                    Line("Total liability of each party is capped at the fees paid in the 12 months before the claim."),
                    Line("Neither party is liable for indirect or consequential losses."),
                ],
                vec![
                    Heading("5. Termination"),
                    Line("Either party may terminate for convenience with ninety (90) days written notice."),
                    Line("Either party may terminate at once for a material breach not cured within thirty (30) days."),
                    Gap,
                    Heading("6. Governing Law and Jurisdiction"),
                    Line("This Agreement is governed by the laws of India."),
                    Line("The courts at Bengaluru, Karnataka have exclusive jurisdiction."),
                    Gap,
                    Line("Signed for the Client by Meera Raghavan, Chief Operating Officer."),
                    Line("Signed for the Service Provider by Daniel Okafor, Managing Partner."),
                ],
            ],
        ),
        pdf_file(
            "contracts/nda-orchid-kestrel.pdf",
            "Mutual Non-Disclosure Agreement: Orchid Biolabs and Kestrel Robotics",
            vec![
                vec![
                    Heading("MUTUAL NON-DISCLOSURE AGREEMENT"),
                    Line("This Agreement is made between Orchid Biolabs Ltd. and Kestrel Robotics GmbH."),
                    Line("It takes effect on 12 January 2026."),
                    Line("Purpose: the parties are evaluating a joint lab automation project."),
                    Line("Each party may disclose designs, protocols and pricing to the other for this purpose."),
                ],
                vec![
                    Heading("1. Obligations"),
                    Line("The recipient keeps Confidential Information secret for five (5) years after disclosure."),
                    Line("The recipient discloses it only to employees who need to know it for the purpose."),
                    Line("On written request, the recipient returns or destroys all materials within fifteen (15) days."),
                ],
                vec![
                    Heading("2. Exclusions"),
                    Line("Obligations do not apply to information that is public or independently developed."),
                    Line("They also do not apply to information lawfully received from a third party."),
                    Gap,
                    Heading("3. Governing Law"),
                    Line("This Agreement is governed by the laws of Germany."),
                    Line("The courts of Munich have exclusive jurisdiction."),
                    Gap,
                    Line("Signed by Dr. Anika Rao for Orchid Biolabs and by Lukas Brandt for Kestrel Robotics."),
                ],
            ],
        ),
        pdf_file(
            "contracts/lease-saffron-tidewater.pdf",
            "Commercial Lease Deed: Saffron Estates and Tidewater Logistics",
            vec![
                vec![
                    Heading("COMMERCIAL LEASE DEED"),
                    Line("Lessor: Saffron Estates Pvt. Ltd."),
                    Line("Lessee: Tidewater Logistics Pvt. Ltd."),
                    Line("Premises: Unit 4B, Harbor Street Warehouse Complex, Chennai."),
                    Line("The premises have a total area of 18,500 square feet."),
                    Line("The lease commences on 1 July 2025."),
                ],
                vec![
                    Heading("1. Rent and Deposit"),
                    Line("Monthly rent is INR 4,85,000, payable by the 5th day of each month."),
                    Line("Rent escalates by 5% every twelve months from the commencement date."),
                    Line("The Lessee pays a security deposit of six months rent, INR 29,10,000, refundable at exit."),
                ],
                vec![
                    Heading("2. Lock-in and Termination"),
                    Line("The lease has a lock-in period of twenty-four (24) months."),
                    Line("After the lock-in, either party may terminate with three (3) months written notice."),
                    Gap,
                    Heading("3. Charges and Disputes"),
                    Line("The Lessee bears electricity and water charges for the premises."),
                    Line("Disputes are resolved by arbitration in Chennai under the Arbitration and Conciliation Act."),
                ],
            ],
        ),
        pdf_file(
            "invoices/inv-2025-0117.pdf",
            "Tax Invoice INV-2025-0117",
            vec![vec![
                Heading("TAX INVOICE"),
                Line("Invoice No: INV-2025-0117"),
                Line("Invoice Date: 16 May 2025"),
                Line("Due Date: 30 June 2025"),
                Line("From: Bluepeak Systems LLP, GSTIN 29AAKFB4821M1Z3"),
                Line("To: Northwind Analytics Pvt. Ltd."),
                Gap,
                table(
                    Some(INVOICE_HEADER),
                    &[
                        ["Data warehouse design sprint", "1", "3,60,000.00", "3,60,000.00"],
                        ["Operations retainer (May 2025)", "1", "2,75,000.00", "2,75,000.00"],
                        ["Cloud cost optimisation review", "1", "40,000.00", "40,000.00"],
                        ["", "", "Subtotal", "6,75,000.00"],
                        ["", "", "IGST 18%", "1,21,500.00"],
                        ["", "", "Total", "7,96,500.00"],
                    ],
                ),
                Gap,
                Line("Total amount due: INR 7,96,500.00"),
                Line("Pay by bank transfer to HDFC Bank, account 50200012345678, IFSC HDFC0001234."),
            ]],
        ),
        pdf_file(
            "invoices/inv-2025-0164.pdf",
            "Tax Invoice INV-2025-0164",
            vec![
                vec![
                    Heading("TAX INVOICE"),
                    Line("Invoice No: INV-2025-0164"),
                    Line("Invoice Date: 18 July 2025"),
                    Line("Due Date: 1 September 2025"),
                    Line("From: Bluepeak Systems LLP, GSTIN 29AAKFB4821M1Z3"),
                    Line("To: Northwind Analytics Pvt. Ltd."),
                    Gap,
                    table(
                        Some(INVOICE_HEADER),
                        &[
                            ["Operations retainer (July 2025)", "1", "2,75,000.00", "2,75,000.00"],
                            ["Dashboard migration (hours)", "40", "2,500.00", "1,00,000.00"],
                            ["Incident response, on-call (hours)", "6", "3,000.00", "18,000.00"],
                        ],
                    ),
                    Gap,
                    Line("Continued on page 2."),
                ],
                vec![
                    Heading("INV-2025-0164 (page 2)"),
                    table(
                        None,
                        &[
                            ["", "", "Subtotal", "3,93,000.00"],
                            ["", "", "IGST 18%", "70,740.00"],
                            ["", "", "Total", "4,63,740.00"],
                        ],
                    ),
                    Gap,
                    Line("Total amount due: INR 4,63,740.00"),
                    Line("Late payment interest of 1.5% per month applies under the Master Services Agreement."),
                ],
            ],
        ),
        pdf_file(
            "invoices/inv-ks-8842.pdf",
            "Invoice KS-8842",
            vec![vec![
                Heading("INVOICE"),
                Line("Invoice number: KS-8842"),
                Line("Date: 3 March 2026"),
                Line("Seller: Kestrel Robotics GmbH, VAT ID DE298765431"),
                Line("Buyer: Orchid Biolabs Ltd."),
                Gap,
                table(
                    Some(["Item", "Qty", "Unit price (EUR)", "Amount (EUR)"]),
                    &[
                        ["Pipetting robot arm PR-200", "2", "7,200.00", "14,400.00"],
                        ["Calibration and installation", "1", "1,850.00", "1,850.00"],
                        ["Operator training (2 days)", "1", "2,200.00", "2,200.00"],
                        ["", "", "Total", "18,450.00"],
                    ],
                ),
                Gap,
                Line("VAT: reverse charge, the buyer accounts for VAT."),
                Line("Total amount due: EUR 18,450.00"),
                Line("Pay within 30 days by SEPA transfer to IBAN DE44 5001 0517 5407 3249 31."),
            ]],
        ),
        text_file(
            "research/chunking-ablation.md",
            "# Chunk size ablation\n\
             \n\
             We measured retrieval recall@10 on an internal set of 400 questions over 1,200 contract pages.\n\
             Embedding model: multilingual-e5-base. The reranker was disabled for this study.\n\
             \n\
             | Chunk size (tokens) | Overlap (tokens) | Recall@10 | MRR |\n\
             |---|---|---|---|\n\
             | 128 | 16 | 0.64 | 0.48 |\n\
             | 256 | 32 | 0.71 | 0.55 |\n\
             | 512 | 64 | 0.78 | 0.61 |\n\
             | 1024 | 128 | 0.74 | 0.57 |\n\
             \n\
             ## Findings\n\
             \n\
             512-token chunks with 64 tokens of overlap gave the best recall@10 (0.78).\n\
             Chunks of 1024 tokens lost precision because unrelated clauses shared a chunk.\n\
             Next step: repeat the study with the cross-encoder reranker enabled.\n",
        ),
        text_file(
            "research/reranker-latency.md",
            "# Reranker latency on CPU\n\
             \n\
             We timed cross-encoder reranking of 50 candidates per query on a 4-core laptop CPU.\n\
             \n\
             | Model | Batch size | Latency per query (ms) | nDCG@10 gain |\n\
             |---|---|---|---|\n\
             | MiniLM-L6 | 16 | 180 | +0.06 |\n\
             | MiniLM-L12 | 16 | 340 | +0.07 |\n\
             | BGE-reranker-base | 8 | 910 | +0.09 |\n\
             \n\
             ## Decision\n\
             \n\
             We ship MiniLM-L6 because it keeps reranking under 200 ms per query.\n\
             The larger models gain little ranking quality for two to five times the latency.\n",
        ),
        text_file(
            "research/ocr-error-study.md",
            "# OCR error rates by scan resolution\n\
             \n\
             We scanned 60 printed invoices at three resolutions and measured the character error rate (CER).\n\
             \n\
             | Resolution (dpi) | CER | Pages per minute |\n\
             |---|---|---|\n\
             | 150 | 4.2% | 38 |\n\
             | 200 | 2.3% | 31 |\n\
             | 300 | 1.1% | 22 |\n\
             \n\
             ## Recommendation\n\
             \n\
             Scan invoices at 300 dpi: the error rate drops below 1.5% while throughput stays above 20 pages per minute.\n",
        ),
        text_file(
            "policies/travel-policy.txt",
            "Travel and Expense Policy (Northwind Analytics)\n\
             Version 3.2, effective 1 January 2026.\n\
             \n\
             Domestic travel per diem is INR 3,500 per day.\n\
             International travel per diem is USD 75 per day.\n\
             Flights under six hours are booked in economy class.\n\
             All travel must be approved in advance by the department head.\n\
             Expense claims must be filed within 30 days of the trip ending.\n",
        ),
        text_file(
            "policies/hybrid-work-policy.txt",
            "Hybrid Work Policy (Northwind Analytics)\n\
             \n\
             Employees work from the office at least three days per week.\n\
             Core collaboration hours are 11:00 to 16:00 India Standard Time.\n\
             Each employee receives a home office equipment stipend of INR 25,000 per year.\n\
             Requests for fully remote work are reviewed by HR every quarter.\n",
        ),
    ]
}

const MSA: &str = "contracts/msa-northwind-bluepeak.pdf";
const NDA: &str = "contracts/nda-orchid-kestrel.pdf";
const LEASE: &str = "contracts/lease-saffron-tidewater.pdf";
const INV_117: &str = "invoices/inv-2025-0117.pdf";
const INV_164: &str = "invoices/inv-2025-0164.pdf";
const INV_KS: &str = "invoices/inv-ks-8842.pdf";
const CHUNKING: &str = "research/chunking-ablation.md";
const RERANKER: &str = "research/reranker-latency.md";
const OCR: &str = "research/ocr-error-study.md";
const TRAVEL: &str = "policies/travel-policy.txt";
const HYBRID: &str = "policies/hybrid-work-policy.txt";

fn source(file: &str, page: Option<u32>, passage: Option<&str>) -> ExpectedSource {
    ExpectedSource {
        file: file.to_string(),
        pages: page.into_iter().collect(),
        passage: passage.map(str::to_string),
    }
}

fn ask(id: &str, question: &str, sources: Vec<ExpectedSource>, facts: &[&str]) -> EvalCase {
    EvalCase {
        id: id.to_string(),
        question: question.to_string(),
        answerable: true,
        sources,
        facts: facts.iter().map(|f| f.to_string()).collect(),
        keep: true,
        context: None,
    }
}

fn unanswerable(id: &str, question: &str) -> EvalCase {
    EvalCase {
        id: id.to_string(),
        question: question.to_string(),
        answerable: false,
        sources: Vec::new(),
        facts: Vec::new(),
        keep: true,
        context: None,
    }
}

fn cases() -> Vec<EvalCase> {
    let s = source;
    vec![
        ask("msa-payment", "Within how many days must Northwind Analytics pay Bluepeak's invoices?",
            vec![s(MSA, Some(2), Some("pay each undisputed invoice within forty-five (45) days of receipt"))], &["45 days"]),
        ask("msa-term", "What is the initial term of the master services agreement between Northwind and Bluepeak?",
            vec![s(MSA, Some(1), Some("initial term of thirty-six (36) months"))], &["36 months"]),
        ask("msa-convenience", "How much notice does the Northwind and Bluepeak agreement require to terminate for convenience?",
            vec![s(MSA, Some(3), Some("terminate for convenience with ninety (90) days written notice"))], &["90 days"]),
        ask("msa-courts", "Which courts have jurisdiction over disputes under the Northwind services agreement?",
            vec![s(MSA, Some(3), Some("The courts at Bengaluru, Karnataka have exclusive jurisdiction."))], &["Bengaluru"]),
        ask("msa-liability", "How is liability capped in the agreement between Northwind and Bluepeak?",
            vec![s(MSA, Some(2), Some("capped at the fees paid in the 12 months before the claim"))], &["12 months"]),
        ask("msa-interest", "What interest is charged on late payments under the Bluepeak services agreement?",
            vec![s(MSA, Some(2), Some("Late payments accrue interest at 1.5% per month"))], &["1.5%"]),
        ask("msa-retainer", "What is the monthly operations retainer under the Northwind master services agreement?",
            vec![s(MSA, Some(2), Some("The monthly operations retainer is INR 2,75,000"))], &["2,75,000"]),
        ask("msa-signatory", "Who signed the services agreement on behalf of Bluepeak Systems?",
            vec![s(MSA, Some(3), Some("Signed for the Service Provider by Daniel Okafor, Managing Partner."))], &["Daniel Okafor"]),
        ask("nda-duration", "For how long must confidential information stay secret under the Orchid and Kestrel NDA?",
            vec![s(NDA, Some(2), Some("keeps Confidential Information secret for five (5) years after disclosure"))], &["5 years"]),
        ask("nda-return", "How quickly must materials be returned on request under the Orchid Biolabs NDA?",
            vec![s(NDA, Some(2), Some("returns or destroys all materials within fifteen (15) days"))], &["15 days"]),
        ask("nda-law", "Which country's law governs the non-disclosure agreement between Orchid Biolabs and Kestrel Robotics?",
            vec![s(NDA, Some(3), Some("This Agreement is governed by the laws of Germany."))], &["Germany"]),
        ask("nda-purpose", "Why are Orchid Biolabs and Kestrel Robotics sharing confidential information?",
            vec![s(NDA, Some(1), Some("evaluating a joint lab automation project"))], &["lab automation"]),
        ask("nda-effective", "When did the Orchid Biolabs and Kestrel Robotics NDA take effect?",
            vec![s(NDA, Some(1), Some("It takes effect on 12 January 2026."))], &["12 January 2026"]),
        ask("lease-rent", "What is the monthly rent for the Harbor Street warehouse unit?",
            vec![s(LEASE, Some(2), Some("Monthly rent is INR 4,85,000"))], &["4,85,000"]),
        ask("lease-deposit", "How large is the security deposit under the Saffron Estates lease?",
            vec![s(LEASE, Some(2), Some("security deposit of six months rent, INR 29,10,000"))], &["29,10,000"]),
        ask("lease-escalation", "By how much does the rent increase each year under the Tidewater Logistics lease?",
            vec![s(LEASE, Some(2), Some("Rent escalates by 5% every twelve months"))], &["5%"]),
        ask("lease-lockin", "What is the lock-in period of the warehouse lease in Chennai?",
            vec![s(LEASE, Some(3), Some("lock-in period of twenty-four (24) months"))], &["24 months"]),
        ask("lease-area", "What is the floor area of the premises leased by Tidewater Logistics?",
            vec![s(LEASE, Some(1), Some("total area of 18,500 square feet"))], &["18,500 square feet"]),
        ask("lease-disputes", "How are disputes under the Saffron Estates lease resolved?",
            vec![s(LEASE, Some(3), Some("Disputes are resolved by arbitration in Chennai"))], &["arbitration"]),
        ask("lease-exit-notice", "How much notice ends the Tidewater lease after the lock-in period?",
            vec![s(LEASE, Some(3), Some("either party may terminate with three (3) months written notice"))], &["3 months"]),
        ask("inv117-total", "What is the total amount due on invoice INV-2025-0117?",
            vec![s(INV_117, Some(1), Some("Total amount due: INR 7,96,500.00"))], &["7,96,500"]),
        ask("inv117-due", "When is invoice INV-2025-0117 due?",
            vec![s(INV_117, Some(1), Some("Due Date: 30 June 2025"))], &["30 June 2025"]),
        ask("inv117-design", "How much did Bluepeak charge for the data warehouse design sprint?",
            vec![s(INV_117, Some(1), None)], &["3,60,000"]),
        ask("bluepeak-gstin", "What is the GSTIN of Bluepeak Systems?",
            vec![
                s(INV_117, Some(1), Some("GSTIN 29AAKFB4821M1Z3")),
                s(INV_164, Some(1), Some("GSTIN 29AAKFB4821M1Z3")),
            ], &["29AAKFB4821M1Z3"]),
        ask("inv164-total", "What is the total of invoice INV-2025-0164?",
            vec![s(INV_164, Some(2), Some("Total amount due: INR 4,63,740.00"))], &["4,63,740"]),
        ask("inv164-hours", "How many hours of dashboard migration were billed on invoice INV-2025-0164?",
            vec![s(INV_164, Some(1), None)], &["40"]),
        ask("inv164-incident", "What did on-call incident response cost on invoice INV-2025-0164?",
            vec![s(INV_164, Some(1), None)], &["18,000"]),
        ask("ks-total", "What is the total of the Kestrel Robotics invoice KS-8842?",
            vec![s(INV_KS, Some(1), Some("Total amount due: EUR 18,450.00"))], &["18,450"]),
        ask("ks-unit-price", "What is the unit price of the PR-200 pipetting robot arm?",
            vec![s(INV_KS, Some(1), None)], &["7,200"]),
        ask("ks-iban", "To which IBAN should invoice KS-8842 be paid?",
            vec![s(INV_KS, Some(1), Some("IBAN DE44 5001 0517 5407 3249 31"))], &["DE44 5001 0517 5407 3249 31"]),
        ask("chunk-best", "Which chunk size gave the best recall@10 in the chunk size ablation?",
            vec![s(CHUNKING, None, Some("512-token chunks with 64 tokens of overlap gave the best recall@10 (0.78)."))], &["512"]),
        ask("chunk-1024", "What recall@10 did 1024-token chunks reach in the ablation?",
            vec![s(CHUNKING, None, None)], &["0.74"]),
        ask("chunk-why", "Why did 1024-token chunks do worse in the chunk size study?",
            vec![s(CHUNKING, None, Some("lost precision because unrelated clauses shared a chunk"))], &["unrelated clauses"]),
        ask("rerank-choice", "Which reranker model was chosen to ship, and why?",
            vec![s(RERANKER, None, Some("We ship MiniLM-L6 because it keeps reranking under 200 ms per query."))], &["MiniLM-L6", "200 ms"]),
        ask("rerank-bge", "How long does the BGE reranker take per query on CPU?",
            vec![s(RERANKER, None, None)], &["910"]),
        ask("ocr-cer-300", "What character error rate did OCR reach at 300 dpi?",
            vec![s(OCR, None, None)], &["1.1%"]),
        ask("ocr-recommendation", "At what resolution should invoices be scanned for OCR?",
            vec![s(OCR, None, Some("Scan invoices at 300 dpi"))], &["300 dpi"]),
        ask("travel-intl", "What is the daily allowance for international travel?",
            vec![s(TRAVEL, None, Some("International travel per diem is USD 75 per day."))], &["USD 75"]),
        ask("travel-class", "In which class are flights under six hours booked?",
            vec![s(TRAVEL, None, Some("Flights under six hours are booked in economy class."))], &["economy"]),
        ask("travel-claims", "How soon after a trip must expense claims be filed?",
            vec![s(TRAVEL, None, Some("Expense claims must be filed within 30 days of the trip ending."))], &["30 days"]),
        ask("hybrid-office", "How many days a week must Northwind employees work from the office?",
            vec![s(HYBRID, None, Some("work from the office at least three days per week"))], &["three days"]),
        ask("hybrid-stipend", "How much is the home office equipment stipend?",
            vec![s(HYBRID, None, Some("home office equipment stipend of INR 25,000 per year"))], &["25,000"]),
        unanswerable("none-parental", "What is Northwind Analytics' parental leave policy?"),
        unanswerable("none-revenue", "What was Bluepeak Systems' annual revenue in 2024?"),
        unanswerable("none-gpu", "Which GPU was used to run the chunk size ablation?"),
        unanswerable("none-warranty", "How long is the warranty on the PR-200 pipetting robot arm?"),
        unanswerable("none-ceo", "Who is the chief executive of Saffron Estates?"),
    ]
}

/// The synthetic corpus and its dataset.
pub fn build() -> SynthCorpus {
    SynthCorpus {
        files: documents(),
        dataset: Dataset {
            id: DATASET_ID.to_string(),
            description: "Synthetic contracts, invoices, research notes and policies (fictional; \
                          generated by `shodh-eval synth`)"
                .to_string(),
            cases: cases(),
        },
    }
}

impl SynthCorpus {
    /// Write `<dir>/corpus/**` and `<dir>/dataset.json`. Refuses when
    /// `<dir>/corpus` holds files this generator does not produce (they
    /// would silently become part of the corpus).
    pub fn write(&self, dir: &Path) -> Result<()> {
        let corpus = dir.join("corpus");
        let produced: BTreeSet<&str> = self.files.iter().map(|f| f.path.as_str()).collect();
        if corpus.exists() {
            let extra: Vec<String> = crate::corpus::supported_files(&corpus)?
                .into_iter()
                .filter(|p| !produced.contains(p.as_str()))
                .collect();
            if !extra.is_empty() {
                bail!(
                    "{} has files the generator does not produce: {}",
                    corpus.display(),
                    extra.join(", ")
                );
            }
        }
        for file in &self.files {
            let path = file
                .path
                .split('/')
                .fold(corpus.clone(), |p, part| p.join(part));
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("creating {}", parent.display()))?;
            }
            std::fs::write(&path, &file.bytes)
                .with_context(|| format!("writing {}", path.display()))?;
        }
        self.dataset.save(&dir.join("dataset.json"))
    }

    /// Check the dataset against the generated text: every expected file
    /// exists, pages are in range, passages are on their page (or in their
    /// file) and every fact is in one of its case's sources.
    pub fn check(&self) -> Result<()> {
        use crate::text::contains_span;
        self.dataset.validate()?;
        let mut problems = Vec::new();
        for file in &self.files {
            if !file.pages.iter().all(|p| p.is_ascii()) {
                problems.push(format!("{} is not ASCII", file.path));
            }
            if file.path.ends_with(".pdf") {
                for page in &file.pages {
                    if let Some(line) = page.lines().find(|l| l.len() > MAX_LINE_CHARS) {
                        problems.push(format!("{}: line too long: {line}", file.path));
                    }
                }
            }
        }
        for case in self.dataset.active_cases() {
            let mut texts: Vec<&str> = Vec::new();
            for src in &case.sources {
                let Some(file) = self.files.iter().find(|f| f.path == src.file) else {
                    problems.push(format!("{}: no file {}", case.id, src.file));
                    continue;
                };
                let scoped: Vec<&str> = if src.pages.is_empty() {
                    file.pages.iter().map(String::as_str).collect()
                } else {
                    let mut out = Vec::new();
                    for &p in &src.pages {
                        match file.pages.get(p as usize - 1) {
                            Some(text) => out.push(text.as_str()),
                            None => {
                                problems.push(format!("{}: {} has no page {p}", case.id, src.file))
                            }
                        }
                    }
                    out
                };
                if let Some(passage) = &src.passage {
                    if !scoped
                        .iter()
                        .any(|t| t.lines().any(|l| contains_span(l, passage)))
                    {
                        problems.push(format!(
                            "{}: passage not on one line of {}: {passage}",
                            case.id, src.file
                        ));
                    }
                }
                texts.extend(scoped);
            }
            for fact in &case.facts {
                if !texts.iter().any(|t| contains_span(t, fact)) {
                    problems.push(format!("{}: fact not in its sources: {fact}", case.id));
                }
            }
        }
        if problems.is_empty() {
            Ok(())
        } else {
            bail!("{}", problems.join("\n"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn committed() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("data")
            .join("synthetic")
    }

    #[test]
    fn dataset_matches_the_generated_text() {
        build().check().unwrap();
    }

    #[test]
    fn generation_is_deterministic() {
        let (a, b) = (build(), build());
        assert_eq!(a.files.len(), b.files.len());
        for (x, y) in a.files.iter().zip(&b.files) {
            assert_eq!(x.bytes, y.bytes, "{}", x.path);
        }
        assert_eq!(a.dataset, b.dataset);
    }

    #[test]
    fn committed_corpus_is_the_generator_output() {
        let corpus = build();
        let root = committed();
        for file in &corpus.files {
            let path = file
                .path
                .split('/')
                .fold(root.join("corpus"), |p, s| p.join(s));
            let on_disk =
                std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            assert!(
                on_disk == file.bytes,
                "{} differs from the generator; run `shodh-eval synth --out crates/shodh-eval/data/synthetic`",
                file.path
            );
        }
        let listed = crate::corpus::supported_files(&root.join("corpus")).unwrap();
        let produced: Vec<String> = {
            let mut p: Vec<String> = corpus.files.iter().map(|f| f.path.clone()).collect();
            p.sort();
            p
        };
        assert_eq!(
            listed, produced,
            "the committed corpus has extra or missing files"
        );
        let dataset = std::fs::read_to_string(root.join("dataset.json")).unwrap();
        assert_eq!(dataset, corpus.dataset.to_json().unwrap());
    }

    #[test]
    fn writing_refuses_unknown_files_in_the_corpus() {
        let dir = tempfile::tempdir().unwrap();
        let corpus = build();
        corpus.write(dir.path()).unwrap();
        std::fs::write(dir.path().join("corpus").join("stray.txt"), "x").unwrap();
        assert!(corpus.write(dir.path()).is_err());
    }
}
