//! A slot in a process-wide stream cap: taken with [`Slot::take`], given back on drop (so also
//! when the task holding it panics).

use tokio::sync::Semaphore;

pub(crate) struct Slot(&'static Semaphore);

impl Slot {
    /// `None` when every slot is taken.
    pub(crate) fn take(cap: &'static Semaphore) -> Option<Slot> {
        cap.try_acquire().ok().map(|p| {
            p.forget();
            Slot(cap)
        })
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.add_permits(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_slot_comes_back_on_drop_and_the_cap_holds() {
        static CAP: Semaphore = Semaphore::const_new(1);
        let a = Slot::take(&CAP).expect("free");
        assert!(Slot::take(&CAP).is_none());
        drop(a);
        assert!(Slot::take(&CAP).is_some());
        assert_eq!(CAP.available_permits(), 1);
    }
}
