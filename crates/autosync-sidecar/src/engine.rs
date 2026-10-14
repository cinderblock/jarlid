//! whisper.cpp's Parakeet runtime, loaded from DLLs at run time.
//!
//! The declarations mirror `include/parakeet.h` and `ggml/include/ggml*.h` of whisper.cpp v1.9.5 —
//! the exact version `build-engine.ps1` pins. Structs passed by value must match field for field,
//! so moving to another whisper.cpp release means re-checking every `#[repr(C)]` here against the
//! new headers.
//!
//! ggml is built with its backends as separate DLLs (one CPU build per instruction-set level, and
//! Vulkan), and Parakeet does not load them itself, so [`Engine::load`] asks ggml to load every
//! backend found beside the engine before creating the model.

use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use autosync::Heard;
use libloading::{Library, Symbol};

#[repr(C)]
#[derive(Clone, Copy)]
struct ContextParams {
    use_gpu: bool,
    gpu_device: c_int,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct TokenData {
    id: i32,
    duration_idx: c_int,
    duration_value: c_int,
    frame_index: c_int,
    p: f32,
    plog: f32,
    /// Hundredths of a second from the start of the samples passed in.
    t0: i64,
    t1: i64,
    is_word_start: bool,
}

type AbortCallback = unsafe extern "C" fn(*mut c_void) -> bool;
type LogCallback = unsafe extern "C" fn(c_int, *const c_char, *mut c_void);

#[repr(C)]
#[derive(Clone, Copy)]
struct FullParams {
    strategy: c_int,
    n_threads: c_int,
    offset_ms: c_int,
    duration_ms: c_int,
    no_context: bool,
    audio_ctx: c_int,
    // Callbacks this sidecar does not use: kept as untyped pointers, which is all the layout needs.
    new_segment_callback: *const c_void,
    new_segment_callback_user_data: *mut c_void,
    new_token_callback: *const c_void,
    new_token_callback_user_data: *mut c_void,
    progress_callback: *const c_void,
    progress_callback_user_data: *mut c_void,
    encoder_begin_callback: *const c_void,
    encoder_begin_callback_user_data: *mut c_void,
    abort_callback: Option<AbortCallback>,
    abort_callback_user_data: *mut c_void,
}

/// The loaded runtime and one model.
pub struct Engine {
    // Declared before the libraries so it is freed while they are still loaded.
    ctx: *mut c_void,
    threads: u32,
    parakeet: Library,
    _ggml: Library,
    _ggml_base: Library,
}

impl Engine {
    /// Load the runtime from `dir` and the model at `model`.
    pub fn load(dir: &Path, model: &Path, gpu: bool, threads: u32) -> Result<Self, String> {
        let open = |name: &str| unsafe {
            Library::new(dir.join(name)).map_err(|e| format!("loading {name}: {e}"))
        };
        // Dependencies first: parakeet.dll links against ggml.dll, which links against
        // ggml-base.dll, and Windows resolves a DLL's imports from what is already loaded before
        // it searches anywhere else.
        let ggml_base = open("ggml-base.dll")?;
        let ggml = open("ggml.dll")?;
        let parakeet = open("parakeet.dll")?;

        unsafe {
            route_logs(&ggml_base, &parakeet);
            let load_all: Symbol<unsafe extern "C" fn(*const c_char)> =
                sym(&ggml, b"ggml_backend_load_all_from_path\0")?;
            let dir_c = c_path(dir)?;
            load_all(dir_c.as_ptr());

            let defaults: Symbol<unsafe extern "C" fn() -> ContextParams> =
                sym(&parakeet, b"parakeet_context_default_params\0")?;
            let init: Symbol<unsafe extern "C" fn(*const c_char, ContextParams) -> *mut c_void> =
                sym(&parakeet, b"parakeet_init_from_file_with_params\0")?;
            let mut params = defaults();
            params.use_gpu = gpu;
            let model_c = c_path(model)?;
            let ctx = init(model_c.as_ptr(), params);
            if ctx.is_null() {
                return Err(format!("could not load model {}", model.display()));
            }
            Ok(Self {
                ctx,
                threads,
                parakeet,
                _ggml: ggml,
                _ggml_base: ggml_base,
            })
        }
    }

    /// The runtime's version string.
    pub fn version(&self) -> String {
        unsafe {
            sym::<unsafe extern "C" fn() -> *const c_char>(&self.parakeet, b"parakeet_version\0")
                .ok()
                .map(|f| CStr::from_ptr(f()).to_string_lossy().into_owned())
                .unwrap_or_default()
        }
    }

    /// The words heard in `samples` (16 kHz mono), timed from the start of `samples`.
    ///
    /// `abort` is polled by the engine during the computation; setting it stops the window early
    /// and returns `Err("aborted")`.
    pub fn hear(&mut self, samples: &[f32], abort: &AtomicBool) -> Result<Vec<Heard>, String> {
        unsafe {
            let defaults: Symbol<unsafe extern "C" fn(c_int) -> FullParams> =
                sym(&self.parakeet, b"parakeet_full_default_params\0")?;
            let full: Symbol<
                unsafe extern "C" fn(*mut c_void, FullParams, *const f32, c_int) -> c_int,
            > = sym(&self.parakeet, b"parakeet_full\0")?;
            let n_segments: Symbol<unsafe extern "C" fn(*mut c_void) -> c_int> =
                sym(&self.parakeet, b"parakeet_full_n_segments\0")?;
            let n_tokens: Symbol<unsafe extern "C" fn(*mut c_void, c_int) -> c_int> =
                sym(&self.parakeet, b"parakeet_full_n_tokens\0")?;
            let token_data: Symbol<unsafe extern "C" fn(*mut c_void, c_int, c_int) -> TokenData> =
                sym(&self.parakeet, b"parakeet_full_get_token_data\0")?;
            let token_str: Symbol<unsafe extern "C" fn(*mut c_void, i32) -> *const c_char> =
                sym(&self.parakeet, b"parakeet_token_to_str\0")?;

            // PARAKEET_SAMPLING_GREEDY, the only strategy.
            let mut params = defaults(0);
            params.n_threads = self.threads as c_int;
            // Each window stands alone; carrying text between them is how repetition starts.
            params.no_context = true;
            params.abort_callback = Some(poll_abort);
            params.abort_callback_user_data = abort as *const AtomicBool as *mut c_void;

            let status = full(self.ctx, params, samples.as_ptr(), samples.len() as c_int);
            if abort.load(Ordering::Relaxed) {
                return Err("aborted".into());
            }
            if status != 0 {
                return Err(format!("parakeet_full failed ({status})"));
            }

            let mut tokens = Vec::new();
            for s in 0..n_segments(self.ctx) {
                for t in 0..n_tokens(self.ctx, s) {
                    let d = token_data(self.ctx, s, t);
                    let text = token_str(self.ctx, d.id);
                    let text = if text.is_null() {
                        String::new()
                    } else {
                        CStr::from_ptr(text).to_string_lossy().into_owned()
                    };
                    tokens.push(Token {
                        text,
                        t0: d.t0,
                        t1: d.t1,
                        word_start: d.is_word_start,
                        p: d.p,
                    });
                }
            }
            Ok(words(&tokens))
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        unsafe {
            if let Ok(free) =
                sym::<unsafe extern "C" fn(*mut c_void)>(&self.parakeet, b"parakeet_free\0")
            {
                free(self.ctx);
            }
        }
    }
}

unsafe extern "C" fn poll_abort(flag: *mut c_void) -> bool {
    (*(flag as *const AtomicBool)).load(Ordering::Relaxed)
}

/// Send both libraries' logs to stderr, so the app can keep or drop them.
unsafe fn route_logs(ggml_base: &Library, parakeet: &Library) {
    unsafe extern "C" fn log(_level: c_int, text: *const c_char, _user: *mut c_void) {
        if !text.is_null() {
            eprint!("{}", CStr::from_ptr(text).to_string_lossy());
        }
    }
    type SetLog = unsafe extern "C" fn(Option<LogCallback>, *mut c_void);
    // ggml_log_set is exported by ggml-base.dll, not ggml.dll: GetProcAddress looks only in the
    // library it is given, never in that library's dependencies.
    if let Ok(set) = sym::<SetLog>(ggml_base, b"ggml_log_set\0") {
        set(Some(log), std::ptr::null_mut());
    }
    if let Ok(set) = sym::<SetLog>(parakeet, b"parakeet_log_set\0") {
        set(Some(log), std::ptr::null_mut());
    }
}

unsafe fn sym<'l, T>(lib: &'l Library, name: &[u8]) -> Result<Symbol<'l, T>, String> {
    lib.get(name).map_err(|e| {
        format!(
            "missing {}: {e}",
            String::from_utf8_lossy(&name[..name.len() - 1])
        )
    })
}

fn c_path(p: &Path) -> Result<CString, String> {
    CString::new(p.to_string_lossy().as_bytes()).map_err(|_| format!("bad path {}", p.display()))
}

/// One subword token as the engine reports it.
#[derive(Debug, Clone)]
pub struct Token {
    /// SentencePiece text: a leading `▁` marks the start of a word.
    pub text: String,
    pub t0: i64,
    pub t1: i64,
    pub word_start: bool,
    pub p: f32,
}

/// Whole words from subword tokens. A word takes its first token's start, its last token's end and
/// its least confident token's probability; punctuation tokens just extend the word before them.
pub fn words(tokens: &[Token]) -> Vec<Heard> {
    let mut out: Vec<Heard> = Vec::new();
    for t in tokens {
        let piece = t.text.trim_start_matches('▁');
        let (start, end) = (t.t0 as f64 / 100.0, t.t1 as f64 / 100.0);
        match out.last_mut() {
            Some(word) if !t.word_start => {
                word.text.push_str(piece);
                word.end = end;
                word.p = word.p.min(t.p);
            }
            _ if piece.is_empty() => {}
            _ => out.push(Heard {
                text: piece.to_string(),
                start,
                end,
                p: t.p,
            }),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token(text: &str, t0: i64, t1: i64, word_start: bool, p: f32) -> Token {
        Token {
            text: text.into(),
            t0,
            t1,
            word_start,
            p,
        }
    }

    #[test]
    fn joins_subwords_into_words() {
        // From parakeet-cli's own example output.
        let w = words(&[
            token("▁And", 24, 56, true, 0.99),
            token("▁so", 56, 88, true, 0.99),
            token(",", 88, 88, false, 0.84),
            token("▁f", 128, 136, true, 0.99),
            token("ell", 136, 152, false, 1.0),
            token("ow", 152, 176, false, 0.95),
        ]);
        let got: Vec<(&str, f64, f64)> = w
            .iter()
            .map(|h| (h.text.as_str(), h.start, h.end))
            .collect();
        assert_eq!(
            got,
            [
                ("And", 0.24, 0.56),
                ("so,", 0.56, 0.88),
                ("fellow", 1.28, 1.76)
            ]
        );
        assert_eq!(w[2].p, 0.95);
    }

    #[test]
    fn struct_layouts_match_the_c_headers() {
        // x86-64: bool and int pad to 8; token data is six 4-byte fields, two int64, a bool.
        assert_eq!(std::mem::size_of::<ContextParams>(), 8);
        assert_eq!(std::mem::size_of::<TokenData>(), 48);
        assert_eq!(std::mem::offset_of!(TokenData, t0), 24);
        assert_eq!(std::mem::offset_of!(FullParams, new_segment_callback), 24);
        assert_eq!(std::mem::size_of::<FullParams>(), 24 + 10 * 8);
    }
}
