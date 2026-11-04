//! Optional file-boundary noise in units of one integer PCM LSB.

use rand_chacha::{ChaCha8Rng, rand_core::SeedableRng};
use rand_distr::{Distribution, Triangular};

use super::{CliError, WavFormat};

#[derive(Clone, Copy, Default)]
pub(super) enum Dither {
    #[default]
    None,
    Tpdf,
}

impl Dither {
    pub(super) fn parse(text: &str) -> Result<Self, CliError> {
        match text {
            "none" => Ok(Self::None),
            "tpdf" => Ok(Self::Tpdf),
            _ => Err(CliError::new(format!(
                "expected dither none or tpdf, got {text}"
            ))),
        }
    }

    pub(super) fn validate(self, format: WavFormat) -> Result<(), CliError> {
        if matches!((self, format), (Self::Tpdf, WavFormat::Float32)) {
            return Err(CliError::new("--dither tpdf requires pcm16 or pcm24"));
        }
        Ok(())
    }

    pub(super) fn generator(self) -> Option<Tpdf> {
        match self {
            Self::None => None,
            Self::Tpdf => Some(Tpdf {
                // Fixed per file, not per block or channel. This is reproducible
                // noise, not a source of cryptographic entropy.
                rng: ChaCha8Rng::from_seed([0x45; 32]),
                distribution: Triangular::new(-1.0, 1.0, 0.0)
                    .expect("the symmetric unit triangular distribution is valid"),
            }),
        }
    }
}

pub(super) struct Tpdf {
    rng: ChaCha8Rng,
    distribution: Triangular<f64>,
}

impl Tpdf {
    pub(super) fn sample_lsb(&mut self) -> f64 {
        // rand_distr supplies the triangular sampler; do not approximate it
        // with a uniform distribution or reuse one draw for both channels.
        self.distribution.sample(&mut self.rng)
    }
}
