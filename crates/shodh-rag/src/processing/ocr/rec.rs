//! Text recognition pre- and post-processing for the PP-OCRv6 recognisers:
//! the character dictionary from the model's `inference.yml`, PaddleOCR's
//! `RecResizeImg` (height 48, width from the batch's widest aspect ratio,
//! normalised to [-1, 1], zero padded on the right) and `CTCLabelDecode`.

use anyhow::{bail, Context, Result};
use image::RgbImage;

use super::geometry::resize_bilinear;

/// Input height of the recognisers.
pub const REC_HEIGHT: u32 = 48;
/// Narrowest input width (`RecResizeImg.image_shape` 3 x 48 x 320): a batch's
/// width is at least this.
pub const MIN_WIDTH: u32 = 320;

/// The CTC classes of a recogniser: index 0 is the blank, then the
/// dictionary in file order, then a space (PaddleOCR's `use_space_char`).
#[derive(Debug, Clone, PartialEq)]
pub struct Charset {
    classes: Vec<String>,
}

impl Charset {
    /// The classes for `dict` (the `character_dict` entries).
    pub fn new(dict: Vec<String>) -> Self {
        let mut classes = Vec::with_capacity(dict.len() + 2);
        classes.push(String::new());
        classes.extend(dict);
        classes.push(" ".to_string());
        Self { classes }
    }

    /// The `PostProcess.character_dict` list of an `inference.yml`. The file
    /// is YAML as PaddleOCR writes it: one `  - <entry>` line per character,
    /// plain or single-quoted (`''` is a quote), directly after
    /// `  character_dict:`. Anything else in the list is rejected rather than
    /// guessed at.
    pub fn from_inference_yml(text: &str) -> Result<Self> {
        let mut lines = text.lines();
        lines
            .by_ref()
            .find(|l| l.trim_end() == "  character_dict:")
            .context("inference.yml has no PostProcess.character_dict")?;
        let mut dict = Vec::new();
        for line in lines {
            let Some(entry) = line.strip_prefix("  - ") else {
                break;
            };
            let value = if let Some(quoted) = entry.strip_prefix('\'') {
                let inner = quoted
                    .strip_suffix('\'')
                    .with_context(|| format!("unterminated quote in dictionary entry {line:?}"))?;
                inner.replace("''", "'")
            } else if entry.starts_with('"') {
                bail!("double-quoted dictionary entries are not supported: {line:?}");
            } else {
                entry.to_string()
            };
            if value.is_empty() {
                bail!("empty dictionary entry: {line:?}");
            }
            dict.push(value);
        }
        if dict.is_empty() {
            bail!("inference.yml has an empty character_dict");
        }
        Ok(Self::new(dict))
    }

    /// Number of CTC classes (dictionary + blank + space).
    pub fn len(&self) -> usize {
        self.classes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.classes.is_empty()
    }
}

/// One recognised line: its text and the mean probability of its characters.
#[derive(Debug, Clone, PartialEq)]
pub struct Recognized {
    pub text: String,
    pub score: f32,
}

/// Greedy CTC decoding of one line's `steps` x `classes` probabilities: the
/// most likely class per step, repeats collapsed, blanks dropped. The score is
/// the mean probability of the kept steps (0 for an empty line).
pub fn ctc_decode(probs: &[f32], steps: usize, charset: &Charset) -> Recognized {
    let classes = charset.len();
    let mut text = String::new();
    let (mut sum, mut kept) = (0.0f32, 0usize);
    let mut previous = usize::MAX;
    for t in 0..steps {
        let row = &probs[t * classes..(t + 1) * classes];
        let (best, p) =
            row.iter()
                .copied()
                .enumerate()
                .fold((0usize, f32::NEG_INFINITY), |acc, (i, v)| {
                    if v > acc.1 {
                        (i, v)
                    } else {
                        acc
                    }
                });
        if best != 0 && best != previous {
            text.push_str(&charset.classes[best]);
            sum += p;
            kept += 1;
        }
        previous = best;
    }
    Recognized {
        text,
        score: if kept == 0 { 0.0 } else { sum / kept as f32 },
    }
}

/// Width of a crop resized to the recogniser's height, keeping its aspect
/// ratio (`ceil(48 * w / h)`).
pub fn scaled_width(crop: &RgbImage) -> u32 {
    let ratio = crop.width() as f32 / crop.height().max(1) as f32;
    ((REC_HEIGHT as f32 * ratio).ceil() as u32).max(1)
}

/// Groups crop indices into batches: crops sorted by aspect ratio (as
/// PaddleOCR does), each batch holding at most `max_batch` crops and at most
/// `width_budget` pixels of padded width (`batch size x widest crop`), so
/// the recogniser's output (`batch x steps x classes` floats) stays bounded
/// however long the lines are. A single crop wider than the budget gets a
/// batch of its own.
pub fn batches(widths: &[u32], max_batch: usize, width_budget: u32) -> Vec<Vec<usize>> {
    let mut order: Vec<usize> = (0..widths.len()).collect();
    order.sort_by_key(|&i| (widths[i], i));
    let mut out: Vec<Vec<usize>> = Vec::new();
    let mut current: Vec<usize> = Vec::new();
    for i in order {
        let widest = widths[i].max(MIN_WIDTH);
        if !current.is_empty()
            && (current.len() >= max_batch || (current.len() as u32 + 1) * widest > width_budget)
        {
            out.push(std::mem::take(&mut current));
        }
        current.push(i);
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

/// The NCHW input for a batch of crops: each resized to height 48 and its
/// own aspect width (capped at the batch width), RGB channels in BGR order,
/// scaled to [-1, 1], zero padded on the right. Returns the data and the
/// batch width.
pub fn batch_input(crops: &[&RgbImage]) -> (Vec<f32>, u32) {
    let width = crops
        .iter()
        .map(|c| scaled_width(c))
        .max()
        .unwrap_or(MIN_WIDTH)
        .max(MIN_WIDTH);
    let plane = (REC_HEIGHT * width) as usize;
    let mut data = vec![0f32; crops.len() * 3 * plane];
    for (b, crop) in crops.iter().enumerate() {
        let w = scaled_width(crop).min(width);
        let resized = resize_bilinear(crop, w, REC_HEIGHT);
        let base = b * 3 * plane;
        for (x, y, px) in resized.enumerate_pixels() {
            let offset = (y * width + x) as usize;
            for (c, channel) in [2usize, 1, 0].into_iter().enumerate() {
                data[base + c * plane + offset] = px.0[channel] as f32 / 127.5 - 1.0;
            }
        }
    }
    (data, width)
}

#[cfg(test)]
mod tests {
    use super::*;

    const YML: &str = "PreProcess:\n  transform_ops:\n  - DecodeImage:\n      img_mode: BGR\nPostProcess:\n  name: CTCLabelDecode\n  character_dict:\n  - '!'\n  - ''''\n  - a\n  - b\n  - ':'\n  - 中\n";

    #[test]
    fn dictionary_is_read_from_inference_yml() {
        let charset = Charset::from_inference_yml(YML).unwrap();
        assert_eq!(charset.len(), 6 + 2);
        assert_eq!(charset.classes[1], "!");
        assert_eq!(charset.classes[2], "'");
        assert_eq!(charset.classes[5], ":");
        assert_eq!(charset.classes[6], "中");
        assert_eq!(charset.classes[7], " ");
    }

    #[test]
    fn malformed_dictionaries_are_rejected() {
        assert!(Charset::from_inference_yml("PostProcess:\n  name: CTCLabelDecode\n").is_err());
        assert!(Charset::from_inference_yml("  character_dict:\n  - 'x\n").is_err());
        assert!(Charset::from_inference_yml("  character_dict:\n  - \"x\"\n").is_err());
        assert!(Charset::from_inference_yml("  character_dict:\nnext: 1\n").is_err());
    }

    fn one_hot(steps: &[(usize, f32)], classes: usize) -> Vec<f32> {
        let mut out = vec![0.0; steps.len() * classes];
        for (t, &(c, p)) in steps.iter().enumerate() {
            out[t * classes + c] = p;
            // The rest of the mass on the blank, unless the blank is chosen.
            if c != 0 {
                out[t * classes] = (1.0 - p).min(p - 0.01);
            }
        }
        out
    }

    #[test]
    fn ctc_collapses_repeats_and_drops_blanks() {
        let charset = Charset::from_inference_yml(YML).unwrap();
        let n = charset.len();
        // a a _ a b _ _ b space a  ->  "aab" + "b a"
        let probs = one_hot(
            &[
                (3, 0.9),
                (3, 0.8),
                (0, 0.9),
                (3, 0.7),
                (4, 0.6),
                (0, 0.9),
                (0, 0.9),
                (4, 1.0),
                (7, 0.5),
                (3, 0.9),
            ],
            n,
        );
        let r = ctc_decode(&probs, 10, &charset);
        assert_eq!(r.text, "aabb a");
        let expected = (0.9 + 0.7 + 0.6 + 1.0 + 0.5 + 0.9) / 6.0;
        assert!((r.score - expected).abs() < 1e-6, "{}", r.score);
        let empty = ctc_decode(&one_hot(&[(0, 0.9), (0, 0.8)], n), 2, &charset);
        assert_eq!(
            empty,
            Recognized {
                text: String::new(),
                score: 0.0
            }
        );
    }

    #[test]
    fn batches_respect_count_and_width_budget() {
        let widths = [100, 2000, 300, 900, 5000, 310];
        let b = batches(&widths, 6, 3200);
        // Sorted: 100(0) 300(2) 310(5) 900(3) 2000(1) 5000(4); four crops
        // padded to 900 would be 3600 px.
        assert_eq!(b, vec![vec![0, 2, 5], vec![3], vec![1], vec![4]]);
        let counted = batches(&[10, 10, 10], 2, 10_000);
        assert_eq!(counted, vec![vec![0, 1], vec![2]]);
        assert!(batches(&[], 6, 3200).is_empty());
    }

    #[test]
    fn batch_input_pads_on_the_right_in_bgr() {
        let narrow = RgbImage::from_pixel(24, 12, image::Rgb([255, 0, 0])); // -> 96 wide
        let wide = RgbImage::from_pixel(400, 40, image::Rgb([0, 0, 255])); // -> 480 wide
        let (data, width) = batch_input(&[&narrow, &wide]);
        assert_eq!(width, 480);
        let plane = 48 * 480;
        // Narrow crop, blue plane (0): blue is 0 -> -1; red plane (2): 255 -> 1.
        assert_eq!(data[0], -1.0);
        assert_eq!(data[2 * plane], 1.0);
        // Padding to the right of its 96 columns stays 0.
        assert_eq!(data[100], 0.0);
        // Wide crop fills the width: blue plane is 1 at its last column.
        assert_eq!(data[3 * plane + 479], 1.0);
    }
}
