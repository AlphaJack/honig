//! `Pkce::new`: a verifier from OS randomness.

use crate::Pkce;
use crate::base64::base64url;

/// Random bytes behind a PKCE verifier: 32 make the 43 characters RFC 7636 asks for at least.
const VERIFIER_BYTES: usize = 32;

impl Pkce {
    /// Creates a pair from 32 bytes of OS randomness.
    /// Errors: the OS has no randomness to give.
    pub fn new() -> Result<Self, getrandom::Error> {
        let mut bytes = [0u8; VERIFIER_BYTES];
        getrandom::fill(&mut bytes)?;
        Ok(Self::from_verifier(&base64url(&bytes)))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn fresh_verifiers_differ() {
        let (a, b) = (crate::Pkce::new(), crate::Pkce::new());
        assert!(a.is_ok() && a != b);
    }
}
