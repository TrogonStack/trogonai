#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NonEmpty<T>(Vec<T>);

impl<T> NonEmpty<T> {
    pub fn one(value: T) -> Self {
        Self(vec![value])
    }

    pub fn from_first(first: T, rest: Vec<T>) -> Self {
        let mut values = Vec::with_capacity(rest.len() + 1);
        values.push(first);
        values.extend(rest);
        Self(values)
    }

    pub fn from_vec(values: Vec<T>) -> Option<Self> {
        if values.is_empty() { None } else { Some(Self(values)) }
    }

    pub fn into_vec(self) -> Vec<T> {
        self.0
    }

    pub fn as_slice(&self) -> &[T] {
        &self.0
    }

    pub fn first(&self) -> &T {
        &self.0[0]
    }

    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.0.iter()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub const fn is_empty(&self) -> bool {
        false
    }

    pub fn map<U, F>(self, f: F) -> NonEmpty<U>
    where
        F: FnMut(T) -> U,
    {
        NonEmpty(self.0.into_iter().map(f).collect())
    }

    pub fn try_map<U, E, F>(self, mut f: F) -> Result<NonEmpty<U>, E>
    where
        F: FnMut(T) -> Result<U, E>,
    {
        let mut values = Vec::with_capacity(self.0.len());
        for value in self.0 {
            values.push(f(value)?);
        }
        Ok(NonEmpty(values))
    }
}

impl<T> IntoIterator for NonEmpty<T> {
    type Item = T;
    type IntoIter = std::vec::IntoIter<T>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}
