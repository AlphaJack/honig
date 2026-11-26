//! What each union that contains itself decided for each value, kept while the outermost value decodes.
//!
//! A union that tries its branches decodes the values nested in it once per branch, and a union nested in such a value
//! does the same: through `n` levels, `2^n` decodes. Each such union records its decision (the branch that won, or the
//! error) per value, so each value is decided once. A value is stored once: an object shares the stored values of its
//! members, so the memory stays that of the body.

use std::any::TypeId;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::hash::{BuildHasher, Hash, Hasher, RandomState};
use std::rc::Rc;

use crate::tagged::Content;

/// A shared value that compares by address.
#[derive(Debug)]
struct Ptr<T>(Rc<T>);

impl<T> Clone for Ptr<T> {
    fn clone(&self) -> Self {
        Self(Rc::clone(&self.0))
    }
}

impl<T> PartialEq for Ptr<T> {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

impl<T> Eq for Ptr<T> {}

impl<T> Ord for Ptr<T> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        Rc::as_ptr(&self.0).cmp(&Rc::as_ptr(&other.0))
    }
}

impl<T> PartialOrd for Ptr<T> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// A stored value. Equal values are one `Rc`, so keys compare by address.
type Key = Ptr<Shape>;

/// A value whose members are stored values.
#[derive(Debug, PartialEq)]
enum Shape {
    Null,
    Bool(bool),
    U64(u64),
    I64(i64),
    /// The bits of the number.
    F64(u64),
    Str(Rc<str>),
    Raw(Rc<str>),
    Seq(Vec<Ptr<Self>>),
    Map(Vec<(Rc<str>, Ptr<Self>)>),
}

/// The values and decisions of the decodes open on this thread.
#[derive(Default)]
struct State {
    /// Keyed at random: a body cannot make its values share a bucket.
    hasher: RandomState,
    /// Each value stored, by its hash.
    values: BTreeMap<u64, Vec<Key>>,
    /// The decision of each union for each value: the index of the branch that won, or the error.
    decisions: BTreeMap<(TypeId, Key), Result<usize, String>>,
    /// How many [`Memo`]s are alive: the state lives while one is.
    open: usize,
}

impl State {
    /// Returns the hash of `c` and its stored value, stored now when it is new.
    fn store(&mut self, c: &Content<'_>) -> (u64, Key) {
        let mut h = self.hasher.build_hasher();
        let shape = match c {
            Content::Null => Shape::Null,
            Content::Bool(b) => Shape::Bool(*b),
            Content::U64(n) => Shape::U64(*n),
            Content::I64(n) => Shape::I64(*n),
            Content::F64(n) => Shape::F64(n.to_bits()),
            Content::Str(s) => Shape::Str(Rc::from(s.as_ref())),
            Content::Raw(s) => Shape::Raw(Rc::from(*s)),
            Content::Seq(items) => Shape::Seq(
                (items.iter())
                    .map(|x| {
                        let (x_hash, key) = self.store(x);
                        h.write_u64(x_hash);
                        key
                    })
                    .collect(),
            ),
            Content::Map(members) => Shape::Map(
                (members.iter())
                    .map(|(name, x)| {
                        let (x_hash, key) = self.store(x);
                        name.hash(&mut h);
                        h.write_u64(x_hash);
                        (Rc::from(name.as_ref()), key)
                    })
                    .collect(),
            ),
        };
        std::mem::discriminant(&shape).hash(&mut h);
        match &shape {
            Shape::Bool(b) => b.hash(&mut h),
            Shape::U64(n) | Shape::F64(n) => n.hash(&mut h),
            Shape::I64(n) => n.hash(&mut h),
            Shape::Str(s) | Shape::Raw(s) => s.hash(&mut h),
            Shape::Null | Shape::Seq(_) | Shape::Map(_) => {}
        }
        let hash = h.finish();
        let bucket = self.values.entry(hash).or_default();
        if let Some(key) = bucket.iter().find(|k| *k.0 == shape) {
            return (hash, key.clone());
        }
        let key = Ptr(Rc::new(shape));
        bucket.push(key.clone());
        (hash, key)
    }
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

/// The decision of union `U` for one value: [`Memo::decided`] finds one made before, [`Memo::decide`] records it.
///
/// The values and decisions last while a `Memo` is alive on the thread, so for one outermost decode, and go with the
/// last one, on an error or a panic too.
#[derive(Debug)]
pub struct Memo {
    at: (TypeId, Key),
}

impl Memo {
    /// Opens the decision of union `U` for `value`.
    #[must_use]
    pub fn of<U: 'static>(value: &Content<'_>) -> Self {
        STATE.with_borrow_mut(|state| {
            let state = state.get_or_insert_with(State::default);
            state.open += 1;
            let (_, key) = state.store(value);
            Self { at: (TypeId::of::<U>(), key) }
        })
    }

    /// Returns the decision made for this value before: the index of the branch that won, or the error.
    #[must_use]
    pub fn decided(&self) -> Option<Result<usize, String>> {
        STATE.with_borrow(|state| state.as_ref()?.decisions.get(&self.at).cloned())
    }

    /// Records the decision for this value.
    pub fn decide(&self, decision: Result<usize, String>) {
        STATE.with_borrow_mut(|state| {
            if let Some(state) = state {
                state.decisions.insert(self.at.clone(), decision);
            }
        });
    }
}

impl Drop for Memo {
    fn drop(&mut self) {
        STATE.with_borrow_mut(|state| {
            if let Some(s) = state {
                s.open -= 1;
                if s.open == 0 {
                    *state = None;
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Equal values are stored once, and an object shares the stored values of its members.
    #[test]
    fn equal_values_are_one() {
        let inner = Content::Map(vec![("sku".into(), Content::Str("BOOK-1".into()))]);
        let outer = Content::Map(vec![("child".into(), inner.clone())]);
        let (a, b) = (Memo::of::<u8>(&inner), Memo::of::<u8>(&outer));
        let Shape::Map(members) = &*b.at.1.0 else { panic!("an object") };
        assert!(members[0].1 == a.at.1, "the member is the stored value");
        a.decide(Ok(1));
        assert_eq!(Memo::of::<u8>(&inner.clone()).decided(), Some(Ok(1)));
        assert_eq!(Memo::of::<u16>(&inner).decided(), None, "another union decides on its own");
    }

    /// The decisions go with the last `Memo` of the thread.
    #[test]
    fn decisions_go_with_the_last_memo() {
        let v = Content::U64(1);
        Memo::of::<u8>(&v).decide(Err("no".to_owned()));
        assert_eq!(Memo::of::<u8>(&v).decided(), None);
        STATE.with_borrow(|state| assert!(state.is_none()));
    }
}
