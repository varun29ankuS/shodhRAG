//! `shodh-eval`: measure shodh's retrieval and grounded answers.
//! See crates/shodh-eval/README.md.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use shodh_eval::corpus::{absolute_dir, default_output_dir, prepare_output_dir, LOCAL_DIR};
use shodh_eval::dataset::Dataset;
use shodh_eval::report::{compare, Baseline, RunReport};
use shodh_eval::{answers, generate, models, ocr, retrieval, scan, synth};
use shodh_rag::config::{OcrConfig, OcrEngineChoice};

const USAGE: &str = "\
shodh-eval: retrieval and answer quality of shodh

USAGE:
  shodh-eval synth     --out <dir>
  shodh-eval synth-scanned --out <dir>
  shodh-eval models    --models <dir> [--answer-check]
  shodh-eval retrieval --corpus <dir> --models <dir> [--dataset <file>] [--runs <n>] [--k <n>]
                       [--out <dir>] [--write-baseline <file>]
                       [--ocr auto|system|ppocr] [--ocr-dpi <n>] [--system-ocr-dpi <n>]
  shodh-eval ocr       --corpus <dir> --truth <ground-truth.json> --engine ppocr|system
                       [--models <dir> | --ocr-model <dir>] [--det-band-pixels <n>] [--dpi <n>]
                       [--out <file>]
  shodh-eval answers   --corpus <dir> --models <dir> [--dataset <file>] [--limit <n>]
                       [--timeout-secs <n>] [--runtime <dir>] [--out <dir>]
  shodh-eval generate  --corpus <dir> --models <dir> [--n <n>] [--seed <n>] [--out <dir>] [--force]
  shodh-eval compare   --baseline <file> --report <file>

Outputs default to .eval-local/runs/<corpus>-<hash>/ (git-ignored). `answers` and
`generate` send passages to the model set by SHODH_EVAL_PROVIDER / SHODH_EVAL_MODEL.";

struct Args {
    values: BTreeMap<String, String>,
    flags: BTreeSet<String>,
}

impl Args {
    fn parse(argv: &[String], value_opts: &[&str], flag_opts: &[&str]) -> Result<Self> {
        let mut values = BTreeMap::new();
        let mut flags = BTreeSet::new();
        let mut iter = argv.iter();
        while let Some(arg) = iter.next() {
            let name = arg
                .strip_prefix("--")
                .ok_or_else(|| anyhow!("unexpected argument '{arg}'"))?;
            if flag_opts.contains(&name) {
                flags.insert(name.to_string());
            } else if value_opts.contains(&name) {
                let value = iter
                    .next()
                    .ok_or_else(|| anyhow!("--{name} needs a value"))?;
                if values.insert(name.to_string(), value.clone()).is_some() {
                    bail!("--{name} is given twice");
                }
            } else {
                bail!("unknown option --{name}");
            }
        }
        Ok(Self { values, flags })
    }

    fn path(&self, name: &str) -> Option<PathBuf> {
        self.values.get(name).map(PathBuf::from)
    }

    fn required_path(&self, name: &str) -> Result<PathBuf> {
        self.path(name)
            .ok_or_else(|| anyhow!("--{name} is required"))
    }

    fn number<T: std::str::FromStr>(&self, name: &str, default: T) -> Result<T> {
        match self.values.get(name) {
            Some(v) => v
                .parse()
                .map_err(|_| anyhow!("--{name} must be a number, got '{v}'")),
            None => Ok(default),
        }
    }

    fn flag(&self, name: &str) -> bool {
        self.flags.contains(name)
    }
}

/// The corpus folder (absolute), the output folder (created, git-ignored)
/// and the dataset (`--dataset`, else the output folder's reviewed
/// `questions.json`).
fn corpus_inputs(args: &Args, need_dataset: bool) -> Result<(PathBuf, PathBuf, Option<Dataset>)> {
    let corpus = absolute_dir(&args.required_path("corpus")?)?;
    let out = args
        .path("out")
        .unwrap_or_else(|| default_output_dir(&corpus));
    prepare_output_dir(&out)?;
    if !need_dataset {
        return Ok((corpus, out, None));
    }
    let dataset_path = match args.path("dataset") {
        Some(p) => p,
        None => {
            let generated = out.join("questions.json");
            if !generated.exists() {
                bail!(
                    "--dataset is required (or generate questions first: {} has no questions.json)",
                    out.display()
                );
            }
            generated
        }
    };
    let dataset = Dataset::load(&dataset_path)?;
    Ok((corpus, out, Some(dataset)))
}

fn write_outputs(out: &Path, report: &RunReport) -> Result<()> {
    let name = report.kind.name();
    let json = out.join(format!("{name}-report.json"));
    let md = out.join(format!("{name}-summary.md"));
    report.write(&json)?;
    let summary = report.markdown();
    std::fs::write(&md, &summary).with_context(|| format!("writing {}", md.display()))?;
    println!("{summary}");
    eprintln!("Wrote {} and {}", json.display(), md.display());
    Ok(())
}

async fn run(command: &str, rest: &[String]) -> Result<ExitCode> {
    match command {
        "synth" => {
            let args = Args::parse(rest, &["out"], &[])?;
            let out = args.required_path("out")?;
            let corpus = synth::build();
            corpus.check()?;
            corpus.write(&out)?;
            eprintln!(
                "Wrote {} files and {} cases to {}",
                corpus.files.len(),
                corpus.dataset.cases.len(),
                out.display()
            );
        }
        "synth-scanned" => {
            let args = Args::parse(rest, &["out"], &[])?;
            let out = args.required_path("out")?;
            let corpus = scan::build()?;
            corpus.check()?;
            scan::write(&corpus, &out)?;
            eprintln!(
                "Wrote {} files and {} cases to {}",
                corpus.files.len(),
                corpus.dataset.cases.len(),
                out.display()
            );
        }
        "ocr" => {
            let args = Args::parse(
                rest,
                &[
                    "corpus",
                    "truth",
                    "engine",
                    "models",
                    "ocr-model",
                    "det-band-pixels",
                    "dpi",
                    "out",
                ],
                &[],
            )?;
            let corpus = absolute_dir(&args.required_path("corpus")?)?;
            let truth = scan::read_ground_truth(&args.required_path("truth")?)?;
            let engine = match args.values.get("engine").map(String::as_str) {
                Some("ppocr") => {
                    let dir = match (args.path("ocr-model"), args.path("models")) {
                        (Some(dir), _) => dir,
                        (None, Some(models)) => shodh_rag::processing::ocr::model_dir(&models),
                        (None, None) => bail!("--models or --ocr-model is required for ppocr"),
                    };
                    ocr::Engine::PpOcr {
                        dir,
                        det_band_pixels: args.number(
                            "det-band-pixels",
                            shodh_rag::processing::ocr::DET_BAND_PIXELS,
                        )?,
                    }
                }
                Some("system") => ocr::Engine::System,
                _ => bail!("--engine must be ppocr or system"),
            };
            let dpi = match args.values.get("dpi") {
                Some(_) => Some(args.number("dpi", 0u32)?),
                None => None,
            };
            let report = ocr::run(&ocr::OcrOptions {
                corpus,
                truth,
                engine,
                dpi,
            })?;
            println!(
                "{} at {} dpi: line recall {:.3}, word recall {:.3}, CER {:.4}, {:.0} ms/page, \
                 load {:?} ms, memory {:?}",
                report.engine,
                report.dpi,
                report.mean_line_recall,
                report.mean_word_recall,
                report.mean_cer,
                report.mean_ms_per_page,
                report.load_ms.map(f64::round),
                report.memory_mb
            );
            if let Some(path) = args.path("out") {
                let json = serde_json::to_string_pretty(&report)? + "\n";
                std::fs::write(&path, json)
                    .with_context(|| format!("writing {}", path.display()))?;
            }
        }
        "models" => {
            let args = Args::parse(rest, &["models"], &["answer-check"])?;
            models::install(&args.required_path("models")?, args.flag("answer-check")).await?;
        }
        "retrieval" => {
            let args = Args::parse(
                rest,
                &[
                    "corpus",
                    "models",
                    "dataset",
                    "runs",
                    "k",
                    "out",
                    "write-baseline",
                    "ocr",
                    "ocr-dpi",
                    "system-ocr-dpi",
                ],
                &[],
            )?;
            let (corpus, out, dataset) = corpus_inputs(&args, true)?;
            let dataset = dataset.ok_or_else(|| anyhow!("no dataset"))?;
            let mut ocr = OcrConfig {
                engine: match args.values.get("ocr").map(String::as_str) {
                    None | Some("auto") => OcrEngineChoice::Auto,
                    Some("system") => OcrEngineChoice::System,
                    Some("ppocr") => OcrEngineChoice::PpOcr,
                    Some(other) => bail!("--ocr must be auto, system or ppocr, got '{other}'"),
                },
                ..OcrConfig::default()
            };
            ocr.render_dpi = args.number("ocr-dpi", ocr.render_dpi)?;
            if args.values.contains_key("system-ocr-dpi") {
                ocr.system_render_dpi = Some(args.number("system-ocr-dpi", 0u32)?);
            }
            let report = retrieval::run(&retrieval::RetrievalOptions {
                corpus,
                dataset,
                models: args.required_path("models")?,
                runs: args.number("runs", 1usize)?,
                k: args.number("k", retrieval::DEFAULT_K)?,
                ocr,
            })
            .await?;
            write_outputs(&out, &report)?;
            if let Some(path) = args.path("write-baseline") {
                Baseline::from_report(&report).write(&path)?;
                eprintln!("Wrote baseline {}", path.display());
            }
        }
        "answers" => {
            let args = Args::parse(
                rest,
                &[
                    "corpus",
                    "models",
                    "dataset",
                    "limit",
                    "timeout-secs",
                    "runtime",
                    "out",
                ],
                &[],
            )?;
            let (corpus, out, dataset) = corpus_inputs(&args, true)?;
            let dataset = dataset.ok_or_else(|| anyhow!("no dataset"))?;
            let runtime_dir = args
                .path("runtime")
                .unwrap_or_else(|| Path::new(LOCAL_DIR).join("runtime"));
            prepare_output_dir(&runtime_dir)?;
            let limit = match args.values.get("limit") {
                Some(_) => Some(args.number("limit", 0usize)?),
                None => None,
            };
            let report = answers::run(&answers::AnswerOptions {
                corpus,
                dataset,
                models: args.required_path("models")?,
                runtime_dir: absolute_dir(&runtime_dir)?,
                limit,
                timeout: Duration::from_secs(
                    args.number("timeout-secs", answers::DEFAULT_TIMEOUT.as_secs())?,
                ),
            })
            .await?;
            write_outputs(&out, &report)?;
        }
        "generate" => {
            let args = Args::parse(rest, &["corpus", "models", "n", "seed", "out"], &["force"])?;
            let (corpus, out, _) = corpus_inputs(&args, false)?;
            let target = out.join("questions.json");
            if target.exists() && !args.flag("force") {
                bail!(
                    "{} exists (it may hold your review); pass --force to replace it",
                    target.display()
                );
            }
            let dataset = generate::run(&generate::GenerateOptions {
                corpus,
                models: args.required_path("models")?,
                n: args.number("n", 50usize)?,
                seed: args.number("seed", 1u64)?,
            })
            .await?;
            dataset.save(&target)?;
            eprintln!(
                "Wrote {} questions to {}. Review them (set \"keep\": false on bad ones), then run \
                 `shodh-eval retrieval --corpus <same folder> --models <dir>`.",
                dataset.cases.len(),
                target.display()
            );
        }
        "compare" => {
            let args = Args::parse(rest, &["baseline", "report"], &[])?;
            let baseline = Baseline::read(&args.required_path("baseline")?)?;
            let report = RunReport::read(&args.required_path("report")?)?;
            let outcome = compare(&baseline, &report);
            println!("{}", outcome.render());
            if !outcome.passed() {
                eprintln!("Regression against the baseline.");
                return Ok(ExitCode::FAILURE);
            }
            eprintln!("No regression against the baseline.");
        }
        "help" | "--help" | "-h" => println!("{USAGE}"),
        other => bail!("unknown command '{other}'\n\n{USAGE}"),
    }
    Ok(ExitCode::SUCCESS)
}

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .init();
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let Some((command, rest)) = argv.split_first() else {
        println!("{USAGE}");
        return ExitCode::FAILURE;
    };
    match run(command, rest).await {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}
