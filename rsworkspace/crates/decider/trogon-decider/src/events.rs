/// A non-empty ordered batch of events.
///
/// The invariant is enforced at construction: every constructor either takes a value
/// directly or returns `Option` when the input might be empty.
///
/// # Example
///
/// ```
/// use trogon_decider::Events;
///
/// #[derive(Debug, PartialEq, Eq)]
/// enum OrderEvent {
///     Placed { order_id: String, customer_id: String },
///     LineItemAdded { sku: String, quantity: u32 },
/// }
///
/// let events = Events::from_first(
///     OrderEvent::Placed {
///         order_id: "order-42".into(),
///         customer_id: "alice".into(),
///     },
///     vec![
///         OrderEvent::LineItemAdded { sku: "sku-1".into(), quantity: 2 },
///         OrderEvent::LineItemAdded { sku: "sku-2".into(), quantity: 1 },
///     ],
/// );
/// assert_eq!(events.len(), 3);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Events<T>(Vec<T>);

impl<T> Events<T> {
    /// Builds a batch containing a single event.
    pub fn one(value: T) -> Self {
        Self(vec![value])
    }

    /// Builds a batch from a guaranteed first event plus zero or more trailing events.
    pub fn from_first(first: T, rest: Vec<T>) -> Self {
        let mut values = Vec::with_capacity(rest.len() + 1);
        values.push(first);
        values.extend(rest);
        Self(values)
    }

    /// Builds a batch from a `Vec`, returning `None` if it is empty.
    pub fn from_vec(values: Vec<T>) -> Option<Self> {
        if values.is_empty() { None } else { Some(Self(values)) }
    }

    /// Consumes the batch and returns the underlying `Vec`.
    pub fn into_vec(self) -> Vec<T> {
        self.0
    }

    /// Appends another batch in order.
    pub fn extend(&mut self, events: Events<T>) {
        self.0.extend(events);
    }

    /// Borrows the events as a slice.
    pub fn as_slice(&self) -> &[T] {
        &self.0
    }

    /// Returns a reference to the first event, which is always present.
    pub fn first(&self) -> &T {
        &self.0[0]
    }

    /// Iterates over the events in order.
    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.0.iter()
    }

    /// Number of events in the batch.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Always `false`; [`Events`] is non-empty by construction.
    ///
    /// Defined so the type satisfies `clippy::len_without_is_empty`.
    pub const fn is_empty(&self) -> bool {
        false
    }

    /// Maps each event through `f`, preserving order and the non-empty invariant.
    pub fn map<U, F>(self, f: F) -> Events<U>
    where
        F: FnMut(T) -> U,
    {
        Events(self.0.into_iter().map(f).collect())
    }

    /// Maps each event through a fallible `f`, short-circuiting on the first error.
    pub fn try_map<U, E, F>(self, mut f: F) -> Result<Events<U>, E>
    where
        F: FnMut(T) -> Result<U, E>,
    {
        let mut values = Vec::with_capacity(self.0.len());
        for value in self.0 {
            values.push(f(value)?);
        }
        Ok(Events(values))
    }
}

impl<T> IntoIterator for Events<T> {
    type Item = T;
    type IntoIter = std::vec::IntoIter<T>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}
