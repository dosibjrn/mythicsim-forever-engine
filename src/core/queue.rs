//! Go `pendingActions` ordering with cancellable handles.
//!
//! Go keeps a sorted slice: earliest time first, then higher priority, then the action
//! inserted first. Cancelling removes an action without disturbing the others. A binary
//! heap with insertion sequence numbers and cancelled slots gives the same pop order. Each
//! entry packs the three keys into one integer, so the heap compares them in one step.

use std::{cmp::Reverse, collections::BinaryHeap};

/// Identifies one scheduled action. A handle is never reused within a queue's lifetime.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Handle(u64);

/// Bits of the packed key below the time: the inverted priority, then the sequence.
const SEQUENCE_BITS: u32 = 40;
const PRIORITY_BITS: u32 = 24;
const PRIORITY_BIAS: i64 = (1 << (PRIORITY_BITS - 1)) - 1;

/// An entry's ordering key, smallest first: the time (biased to sort as unsigned), the
/// priority inverted so a higher one sorts first, and the insertion sequence.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Entry(u128);

impl Entry {
    fn new(time: i64, priority: i32, sequence: u64) -> Self {
        let inverted = PRIORITY_BIAS - i64::from(priority);
        assert!(
            (0..1 << PRIORITY_BITS).contains(&inverted),
            "action priority out of range"
        );
        assert!(sequence < 1 << SEQUENCE_BITS, "too many scheduled actions");
        let time = (time as u64) ^ (1 << 63);
        Entry(
            (u128::from(time) << (PRIORITY_BITS + SEQUENCE_BITS))
                | ((inverted as u128) << SEQUENCE_BITS)
                | u128::from(sequence),
        )
    }

    fn time(self) -> i64 {
        ((self.0 >> (PRIORITY_BITS + SEQUENCE_BITS)) as u64 ^ (1 << 63)) as i64
    }

    fn sequence(self) -> u64 {
        (self.0 & ((1 << SEQUENCE_BITS) - 1)) as u64
    }
}

pub(crate) struct PendingQueue<T> {
    /// The heap pops its greatest entry, so it holds them reversed.
    heap: BinaryHeap<Reverse<Entry>>,
    /// Payloads by sequence offset; `None` once popped or cancelled.
    slots: Vec<Option<T>>,
    base: u64,
    next: u64,
}

impl<T> Default for PendingQueue<T> {
    fn default() -> Self {
        Self {
            heap: BinaryHeap::new(),
            slots: Vec::new(),
            base: 0,
            next: 0,
        }
    }
}

impl<T> PendingQueue<T> {
    /// Remove every action. Handles from before the clear stay invalid.
    pub(crate) fn clear(&mut self) {
        self.heap.clear();
        self.slots.clear();
        self.base = self.next;
    }

    pub(crate) fn push(&mut self, time: i64, priority: i32, payload: T) -> Handle {
        let sequence = self.next;
        self.next += 1;
        self.heap
            .push(Reverse(Entry::new(time, priority, sequence)));
        self.slots.push(Some(payload));
        Handle(sequence)
    }

    fn slot(&mut self, handle: Handle) -> Option<&mut Option<T>> {
        let index = handle.0.checked_sub(self.base)?;
        self.slots.get_mut(usize::try_from(index).ok()?)
    }

    /// Cancel a pending action. Returns false if it already ran or was cancelled.
    pub(crate) fn cancel(&mut self, handle: Handle) -> bool {
        self.slot(handle).and_then(Option::take).is_some()
    }

    /// The time of the next live action, dropping cancelled entries on the way.
    pub(crate) fn peek_time(&mut self) -> Option<i64> {
        while let Some(&Reverse(entry)) = self.heap.peek() {
            let index = (entry.sequence() - self.base) as usize;
            if self.slots[index].is_some() {
                return Some(entry.time());
            }
            self.heap.pop();
        }
        None
    }

    /// Pop only a permitted passive event through the given clock, leaving every
    /// unrelated pending action at its original priority and stable handle.
    /// This lets a retained world actor pay native regeneration ticks while its
    /// defeated target is absent, without executing its queued combat actions.
    pub(crate) fn pop_next_matching_through<F>(
        &mut self,
        through: i64,
        mut matches: F,
    ) -> Option<(i64, Handle, T)>
    where
        F: FnMut(&T) -> bool,
    {
        let mut held = Vec::new();
        let mut result = None;
        while let Some(Reverse(entry)) = self.heap.pop() {
            let index = (entry.sequence() - self.base) as usize;
            let Some(payload) = self.slots[index].as_ref() else {
                continue; // Already cancelled: keep the queue compact.
            };
            if entry.time() > through {
                held.push(Reverse(entry));
                break;
            }
            if matches(payload) {
                let payload = self.slots[index].take().expect("live queued action");
                result = Some((entry.time(), Handle(entry.sequence()), payload));
                break;
            }
            held.push(Reverse(entry));
        }
        self.heap.extend(held);
        result
    }

    /// Pop the next live action.
    pub(crate) fn pop(&mut self) -> Option<(i64, Handle, T)> {
        while let Some(Reverse(entry)) = self.heap.pop() {
            let index = (entry.sequence() - self.base) as usize;
            if let Some(payload) = self.slots[index].take() {
                return Some((entry.time(), Handle(entry.sequence()), payload));
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passive_recovery_does_not_pop_combat_or_invalidate_its_handles() {
        let mut queue = PendingQueue::default();
        let attack = queue.push(10, 2, "attack");
        queue.push(12, 1, "mana");
        let cooldown = queue.push(13, 3, "cooldown");
        queue.push(14, 1, "health");
        queue.push(20, 1, "future_mana");

        let first = queue.pop_next_matching_through(15, |action| {
            matches!(*action, "mana" | "health")
        });
        assert_eq!(first.map(|(at, _, action)| (at, action)), Some((12, "mana")));
        let second = queue.pop_next_matching_through(15, |action| {
            matches!(*action, "mana" | "health")
        });
        assert_eq!(second.map(|(at, _, action)| (at, action)), Some((14, "health")));
        assert!(queue.pop_next_matching_through(15, |action| {
            matches!(*action, "mana" | "health")
        }).is_none());
        assert!(queue.cancel(attack));
        assert!(queue.cancel(cooldown));
        assert_eq!(queue.pop().map(|(at, _, action)| (at, action)), Some((20, "future_mana")));
        assert!(queue.pop().is_none());
    }

    #[test]
    fn pops_by_time_then_priority_then_insertion_like_go() {
        let mut queue = PendingQueue::default();
        queue.push(10, 0, "first_gcd");
        queue.push(10, 1, "regen");
        let cancelled = queue.push(10, 3, "cancelled_dot");
        queue.push(10, 0, "second_gcd");
        queue.push(5, -1, "earlier");
        assert!(queue.cancel(cancelled));
        assert!(!queue.cancel(cancelled));
        let order: Vec<_> = std::iter::from_fn(|| queue.pop().map(|(_, _, p)| p)).collect();
        assert_eq!(order, ["earlier", "regen", "first_gcd", "second_gcd"]);
    }

    #[test]
    fn packed_keys_order_negative_and_extreme_times() {
        let mut queue = PendingQueue::default();
        queue.push(i64::MAX, 0, "never");
        queue.push(-3_000_000_000, 0, "prepull");
        queue.push(0, -5, "low");
        queue.push(0, 5, "high");
        let order: Vec<_> = std::iter::from_fn(|| queue.pop().map(|(t, _, p)| (t, p))).collect();
        assert_eq!(
            order,
            [
                (-3_000_000_000, "prepull"),
                (0, "high"),
                (0, "low"),
                (i64::MAX, "never")
            ]
        );
    }

    #[test]
    fn handles_from_before_a_clear_are_inert() {
        let mut queue = PendingQueue::default();
        let old = queue.push(1, 0, 1);
        queue.clear();
        let new = queue.push(1, 0, 2);
        assert!(!queue.cancel(old));
        assert_eq!(queue.pop().map(|(_, _, p)| p), Some(2));
        assert!(!queue.cancel(new));
    }
}
