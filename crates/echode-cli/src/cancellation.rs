use std::io::Read;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::CliError;

/// Opt-in CLI control, independent of advisory stdout progress. Never read on an
/// audio callback: the command checks this flag between blocks and before rename.
#[derive(Default)]
pub(crate) struct Cancellation(Arc<AtomicBool>);

impl Cancellation {
    pub(crate) fn from_stdin() -> Result<Self, CliError> {
        let cancellation = Self::default();
        let requested = Arc::clone(&cancellation.0);
        std::thread::Builder::new()
            .name("export-cancellation".into())
            .spawn(move || {
                let mut byte = [0];
                loop {
                    match std::io::stdin().read(&mut byte) {
                        Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                        // Any byte, EOF, or a broken control channel requests cancellation.
                        _ => break,
                    }
                }
                requested.store(true, Ordering::Release);
            })
            .map_err(|error| {
                CliError::new(format!("cannot monitor export cancellation: {error}"))
            })?;
        // The stdin reader is process-scoped, not joined on success: a controller
        // may keep the pipe open until it observes the CLI's exit.
        Ok(cancellation)
    }

    pub(crate) fn check(&self) -> Result<(), CliError> {
        if self.0.load(Ordering::Acquire) {
            Err(CliError::cancelled())
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Dither, WavFile, WavFormat};

    #[test]
    fn cancellation_before_publication_keeps_the_original_and_removes_the_partial() {
        let output =
            std::env::temp_dir().join(format!("echode-cancel-publish-{}.wav", std::process::id()));
        std::fs::write(&output, b"original").unwrap();
        let mut wav =
            WavFile::create_with_dither(&output, 48_000, 1, WavFormat::Pcm24, Dither::None)
                .unwrap();
        let partial = wav.temporary.clone();
        wav.write_block(&[0.5, 0.5]).unwrap();
        let cancellation = Cancellation::default();
        cancellation.0.store(true, Ordering::Release);
        let error = wav.finish_cancellable(&cancellation).unwrap_err();
        assert!(error.is_cancelled());
        assert_eq!(std::fs::read(&output).unwrap(), b"original");
        assert!(!partial.exists());
        std::fs::remove_file(output).unwrap();
    }
}
