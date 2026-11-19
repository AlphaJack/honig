//! `Pkce::new`: a verifier from OS randomness.

use super::NoRandomness;
use crate::Pkce;
use crate::base64::base64url;

/// Random bytes behind a PKCE verifier: 32 make the 43 characters RFC 7636 asks for at least.
const VERIFIER_BYTES: usize = 32;

impl Pkce {
    /// Creates a pair from 32 bytes of OS randomness.
    /// Errors: the OS has no randomness to give.
    pub fn new() -> Result<Self, NoRandomness> {
        let mut bytes = [0u8; VERIFIER_BYTES];
        getrandom::fill(&mut bytes).map_err(|e| NoRandomness(Box::new(e)))?;
        Ok(Self::from_verifier(&base64url(&bytes)))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn fresh_verifiers_differ() {
        let (a, b) = (crate::Pkce::new(), crate::Pkce::new());
        assert_ne!(a.unwrap(), b.unwrap());
    }

    /// The error is the runtime's own: a new major version of `getrandom` does not change the signature.
    #[test]
    fn the_error_names_no_dependency() {
        let made: Result<crate::Pkce, crate::NoRandomness> = crate::Pkce::new();
        assert!(made.is_ok());
        let error = super::NoRandomness(Box::new(getrandom::Error::UNSUPPORTED));
        assert!(std::error::Error::source(&error).is_some());
        assert_eq!(error.to_string(), "the OS gave no randomness for a PKCE verifier");
    }
}
