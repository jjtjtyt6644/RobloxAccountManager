//! DPAPI wrappers. Flags = 0 => CURRENT USER scope (never machine scope).
use windows::Win32::Foundation::{LocalFree, HLOCAL};
use windows::Win32::Security::Cryptography::{CryptProtectData, CryptUnprotectData, CRYPT_INTEGER_BLOB};
use zeroize::Zeroizing;

pub fn protect(plain: &[u8]) -> Result<Vec<u8>, String> {
    unsafe {
        let input = CRYPT_INTEGER_BLOB { cbData: plain.len() as u32, pbData: plain.as_ptr() as *mut u8 };
        let mut out = CRYPT_INTEGER_BLOB::default();
        CryptProtectData(&input, None, None, None, None, 0, &mut out).map_err(|e| e.to_string())?;
        let v = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
        let _ = LocalFree(HLOCAL(out.pbData as *mut _));
        Ok(v)
    }
}

pub fn unprotect(blob: &[u8]) -> Result<Zeroizing<Vec<u8>>, String> {
    unsafe {
        let input = CRYPT_INTEGER_BLOB { cbData: blob.len() as u32, pbData: blob.as_ptr() as *mut u8 };
        let mut out = CRYPT_INTEGER_BLOB::default();
        CryptUnprotectData(&input, None, None, None, None, 0, &mut out).map_err(|e| e.to_string())?;
        let s = std::slice::from_raw_parts_mut(out.pbData, out.cbData as usize);
        let v = Zeroizing::new(s.to_vec());
        s.fill(0); // wipe the OS-allocated plaintext before freeing
        let _ = LocalFree(HLOCAL(out.pbData as *mut _));
        Ok(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roundtrip() {
        let c = protect(b"secret-cookie").unwrap();
        assert_ne!(c, b"secret-cookie");
        assert_eq!(&**unprotect(&c).unwrap(), b"secret-cookie");
    }
}
