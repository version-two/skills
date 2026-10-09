use zeroize::Zeroize;

const MASK: &[u8] = b"***";
const MIN_LEN: usize = 4;

/// Streaming replacement of secret values, safe across chunk boundaries: a tail that could be the
/// start of a secret is held back until the next chunk (or `finish`).
pub struct Redactor {
    secrets: Vec<Vec<u8>>,
    held: Vec<u8>,
}

impl Redactor {
    pub fn new<'a>(values: impl IntoIterator<Item = &'a str>) -> Redactor {
        let mut secrets: Vec<Vec<u8>> =
            values.into_iter().filter(|v| v.len() >= MIN_LEN).map(|v| v.as_bytes().to_vec()).collect();
        secrets.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
        secrets.dedup();
        Redactor { secrets, held: Vec::new() }
    }

    pub fn is_empty(&self) -> bool {
        self.secrets.is_empty()
    }

    pub fn push(&mut self, chunk: &[u8]) -> Vec<u8> {
        if self.secrets.is_empty() {
            return chunk.to_vec();
        }
        let mut buffer = std::mem::take(&mut self.held);
        buffer.extend_from_slice(chunk);
        self.scan(buffer, false)
    }

    pub fn finish(&mut self) -> Vec<u8> {
        let buffer = std::mem::take(&mut self.held);
        self.scan(buffer, true)
    }

    fn scan(&mut self, mut buffer: Vec<u8>, last: bool) -> Vec<u8> {
        let mut out = Vec::with_capacity(buffer.len());
        let mut i = 0;
        while i < buffer.len() {
            let rest = &buffer[i..];
            let could_grow = !last && self.secrets.iter().any(|s| s.len() > rest.len() && s.starts_with(rest));
            if could_grow {
                self.held = rest.to_vec();
                break;
            }
            match self.secrets.iter().find(|s| rest.starts_with(s)) {
                Some(secret) => {
                    out.extend_from_slice(MASK);
                    i += secret.len();
                }
                None => {
                    out.push(buffer[i]);
                    i += 1;
                }
            }
        }
        buffer.zeroize();
        out
    }
}

impl Drop for Redactor {
    fn drop(&mut self) {
        for secret in &mut self.secrets {
            secret.zeroize();
        }
        self.held.zeroize();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(values: &[&str], chunks: &[&str]) -> String {
        let mut r = Redactor::new(values.iter().copied());
        let mut out = Vec::new();
        for c in chunks {
            out.extend(r.push(c.as_bytes()));
        }
        out.extend(r.finish());
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn replaces_inside_a_chunk() {
        assert_eq!(run(&["hunter22"], &["pass is hunter22, ok hunter22\n"]), "pass is ***, ok ***\n");
    }

    #[test]
    fn replaces_across_chunk_boundaries() {
        assert_eq!(run(&["hunter22"], &["pass is hun", "ter", "22 done"]), "pass is *** done");
    }

    #[test]
    fn a_held_prefix_that_never_completes_is_released() {
        assert_eq!(run(&["hunter22"], &["almost hunt", "ing"]), "almost hunting");
        assert_eq!(run(&["hunter22"], &["ends with hunt"]), "ends with hunt");
    }

    #[test]
    fn longest_secret_wins_and_short_values_are_ignored() {
        assert_eq!(run(&["abcd", "abcdef", "xy"], &["abcdef xy abcd"]), "*** xy ***");
    }

    #[test]
    fn no_secrets_is_a_passthrough() {
        let mut r = Redactor::new([]);
        assert_eq!(r.push(b"anything"), b"anything");
        assert!(r.is_empty());
    }
}
