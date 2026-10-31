use std::ops::{Deref, DerefMut};
use std::sync::Mutex;

use crate::wire::noise::MAX_RECORD;

pub const BIG: usize = MAX_RECORD + 2;
pub const KEPT: usize = 64;

static FREE: Mutex<Vec<Box<[u8]>>> = Mutex::new(Vec::new());

pub struct Big(Option<Box<[u8]>>);

impl Big {
    pub fn take() -> Big {
        let reused = FREE.lock().map(|mut f| f.pop()).unwrap_or(None);
        Big(Some(
            reused.unwrap_or_else(|| vec![0u8; BIG].into_boxed_slice()),
        ))
    }
}

impl Drop for Big {
    fn drop(&mut self) {
        if let Some(b) = self.0.take()
            && let Ok(mut free) = FREE.lock()
            && free.len() < KEPT
        {
            free.push(b);
        }
    }
}

impl Deref for Big {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        self.0
            .as_deref()
            .expect("a pooled buffer is held until drop")
    }
}

impl DerefMut for Big {
    fn deref_mut(&mut self) -> &mut [u8] {
        self.0
            .as_deref_mut()
            .expect("a pooled buffer is held until drop")
    }
}

#[cfg(test)]
pub fn kept() -> usize {
    FREE.lock().map(|f| f.len()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffers_are_full_records_and_the_pool_never_keeps_more_than_its_bound() {
        let many: Vec<Big> = (0..KEPT + 8).map(|_| Big::take()).collect();
        assert!(many.iter().all(|b| b.len() == BIG));
        drop(many);
        assert!(kept() <= KEPT);
        let again = Big::take();
        assert_eq!(again.len(), BIG);
    }
}
