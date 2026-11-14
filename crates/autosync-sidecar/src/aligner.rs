//! The forced-alignment model: audio in, a character emission out.
//!
//! Meta's MMS forced-alignment network (wav2vec 2.0, 1,130 languages), exported to ONNX with its
//! matrix multiplies quantized to int8, run by ONNX Runtime. Both are optional downloads: the
//! runtime (`onnxruntime.dll`) is loaded from the engine directory at run time rather than
//! linked, so this executable starts and hears without it. See
//! `plans/word-aligner-in-app.md`.
//!
//! The exported graph is the bare network: the waveform normalisation before it and the
//! log-softmax and star column after it are done here, as torchaudio's `MMS_FA` bundle does
//! them.

use std::path::Path;
use std::sync::OnceLock;

use ort::session::Session;
use ort::value::Tensor;

/// The shortest stretch of audio worth aligning: 0.1 s (five frames).
pub const MIN_SAMPLES: usize = 1600;

pub struct Aligner {
    session: Session,
}

/// ONNX Runtime's environment is process-wide and can be set up once.
static RUNTIME: OnceLock<Result<ort::environment::Environment, String>> = OnceLock::new();

impl Aligner {
    /// Load ONNX Runtime from `dir` and the model at `model`, running on `threads` CPU threads.
    pub fn load(dir: &Path, model: &Path, threads: u32) -> Result<Self, String> {
        let env = RUNTIME
            .get_or_init(|| {
                ort::init_from(dir.join("onnxruntime.dll"))
                    .map_err(|e| format!("loading onnxruntime.dll: {e}"))?
                    .with_name("jarlid-aligner")
                    .build()
                    .map_err(|e| e.to_string())
            })
            .as_ref()
            .map_err(Clone::clone)?;
        let session = Session::builder(env)
            .map_err(|e| e.to_string())?
            .with_intra_threads(threads.max(1) as usize)
            .map_err(|e| e.to_string())?
            .commit_from_file(model)
            .map_err(|e| format!("could not load aligner model {}: {e}", model.display()))?;
        Ok(Self { session })
    }

    /// The emission for `samples` (16 kHz mono): `frames × autosync::ctc::CLASSES`
    /// log-probabilities, star column included. About 50 frames a second.
    pub fn emit(&mut self, samples: &[f32]) -> Result<Vec<f32>, String> {
        let input = layer_norm(samples);
        let tensor =
            Tensor::from_array(([1usize, input.len()], input)).map_err(|e| e.to_string())?;
        let outputs = self
            .session
            .run(ort::inputs!["audio" => tensor])
            .map_err(|e| e.to_string())?;
        let (shape, logits) = outputs["emission"]
            .try_extract_tensor::<f32>()
            .map_err(|e| e.to_string())?;
        if shape.len() != 3 || shape[2] as usize != autosync::ctc::MODEL_CLASSES {
            return Err(format!("unexpected emission shape {shape:?}"));
        }
        Ok(autosync::ctc::with_star(logits))
    }
}

/// Zero mean and unit variance over the whole input, as `torch.nn.functional.layer_norm` with no
/// weights (eps 1e-5) does: the normalisation `MMS_FA` applies before its network.
fn layer_norm(x: &[f32]) -> Vec<f32> {
    let n = x.len().max(1) as f64;
    let mean = x.iter().map(|&v| v as f64).sum::<f64>() / n;
    let var = x.iter().map(|&v| (v as f64 - mean).powi(2)).sum::<f64>() / n;
    let scale = 1.0 / (var + 1e-5).sqrt();
    x.iter()
        .map(|&v| ((v as f64 - mean) * scale) as f32)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layer_norm_centres_and_scales() {
        let y = layer_norm(&[1.0, 2.0, 3.0, 4.0]);
        let mean: f32 = y.iter().sum::<f32>() / 4.0;
        let var: f32 = y.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / 4.0;
        assert!(mean.abs() < 1e-6);
        assert!((var - 1.0).abs() < 1e-4);
    }
}
