//! Per-user DPAPI protection for artwork cache entries belonging to private repositories.

const MAX_PLAINTEXT_BYTES: usize = 16 * 1024 * 1024;
const MAX_CIPHERTEXT_BYTES: usize = MAX_PLAINTEXT_BYTES + 64 * 1024;
const MAX_ENTROPY_BYTES: usize = 4096;

#[cfg(windows)]
mod imp {
    use super::*;
    use std::{
        ptr,
        sync::atomic::{Ordering, compiler_fence},
    };
    use windows_sys::Win32::{
        Foundation::{HLOCAL, LocalFree},
        Security::Cryptography::{
            CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
        },
    };

    struct ProtectedBuffer(CRYPT_INTEGER_BLOB);

    impl Drop for ProtectedBuffer {
        fn drop(&mut self) {
            let blob = &mut self.0;
            if !blob.pbData.is_null() {
                let size = blob.cbData as usize;
                for index in 0..size {
                    // DPAPI-owned plaintext/ciphertext is cleared before LocalFree.
                    unsafe { ptr::write_volatile(blob.pbData.add(index), 0) };
                }
                compiler_fence(Ordering::SeqCst);
                unsafe { LocalFree(blob.pbData.cast::<core::ffi::c_void>() as HLOCAL) };
                blob.pbData = ptr::null_mut();
                blob.cbData = 0;
            }
        }
    }

    fn transform(input: &[u8], entropy: &[u8], protect: bool) -> Option<Vec<u8>> {
        let input_limit = if protect {
            MAX_PLAINTEXT_BYTES
        } else {
            MAX_CIPHERTEXT_BYTES
        };
        let output_limit = if protect {
            MAX_CIPHERTEXT_BYTES
        } else {
            MAX_PLAINTEXT_BYTES
        };
        if input.is_empty()
            || input.len() > input_limit
            || entropy.is_empty()
            || entropy.len() > MAX_ENTROPY_BYTES
        {
            return None;
        }

        let input_blob = CRYPT_INTEGER_BLOB {
            cbData: u32::try_from(input.len()).ok()?,
            pbData: input.as_ptr().cast_mut(),
        };
        let entropy_blob = CRYPT_INTEGER_BLOB {
            cbData: u32::try_from(entropy.len()).ok()?,
            pbData: entropy.as_ptr().cast_mut(),
        };
        let mut output = ProtectedBuffer(CRYPT_INTEGER_BLOB::default());
        let ok = unsafe {
            if protect {
                CryptProtectData(
                    &input_blob,
                    ptr::null(),
                    &entropy_blob,
                    ptr::null(),
                    ptr::null(),
                    CRYPTPROTECT_UI_FORBIDDEN,
                    &mut output.0,
                )
            } else {
                CryptUnprotectData(
                    &input_blob,
                    ptr::null_mut(),
                    &entropy_blob,
                    ptr::null(),
                    ptr::null(),
                    CRYPTPROTECT_UI_FORBIDDEN,
                    &mut output.0,
                )
            }
        };
        if ok == 0
            || output.0.pbData.is_null()
            || output.0.cbData == 0
            || output.0.cbData as usize > output_limit
        {
            return None;
        }

        Some(unsafe {
            std::slice::from_raw_parts(output.0.pbData, output.0.cbData as usize).to_vec()
        })
    }

    pub(crate) fn protect(bytes: &[u8], entropy: &[u8]) -> Option<Vec<u8>> {
        transform(bytes, entropy, true)
    }

    pub(crate) fn unprotect(cipher: &[u8], entropy: &[u8]) -> Option<Vec<u8>> {
        transform(cipher, entropy, false)
    }
}

#[cfg(windows)]
pub(crate) use imp::{protect, unprotect};

#[cfg(not(windows))]
pub(crate) fn protect(_bytes: &[u8], _entropy: &[u8]) -> Option<Vec<u8>> {
    None
}

#[cfg(not(windows))]
pub(crate) fn unprotect(_cipher: &[u8], _entropy: &[u8]) -> Option<Vec<u8>> {
    None
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn private_artwork_round_trips_for_the_same_entropy() {
        let plaintext = b"private-artwork-fixture-7db0e38c";
        let entropy = b"profile-and-repository-scope-fixture";
        let protected = protect(plaintext, entropy).expect("DPAPI protection should succeed");

        assert_ne!(protected, plaintext);
        assert!(
            !protected
                .windows(plaintext.len())
                .any(|window| window == plaintext)
        );
        assert_eq!(
            unprotect(&protected, entropy).as_deref(),
            Some(plaintext.as_slice())
        );
    }

    #[test]
    fn private_artwork_rejects_different_entropy() {
        let protected = protect(b"private-artwork-fixture", b"profile-a")
            .expect("DPAPI protection should succeed");

        assert!(unprotect(&protected, b"profile-b").is_none());
    }

    #[test]
    fn private_artwork_rejects_oversized_inputs() {
        let oversized_plaintext = vec![0; MAX_PLAINTEXT_BYTES + 1];
        let oversized_ciphertext = vec![0; MAX_CIPHERTEXT_BYTES + 1];

        assert!(protect(&oversized_plaintext, b"scope").is_none());
        assert!(unprotect(&oversized_ciphertext, b"scope").is_none());
        assert!(protect(b"data", &vec![0; MAX_ENTROPY_BYTES + 1]).is_none());
    }
}
